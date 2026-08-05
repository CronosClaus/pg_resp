// Demo 3 — rate limiter. The HONESTY demo (bible §11.3).
//
// This one exists to find the point where pg_resp stops being the right answer.
// A fixed-window rate limiter is the most write-heavy, least cacheable thing you
// can ask a RESP server to do: every single request is an INCR, there is no read
// path to amortise, and the whole workload is the per-operation cost. If Redis
// wins anywhere, it wins here, and the deliverable is the crossover number:
// "above X checks/sec of pure rate limiting, keep Redis; below it, the second
// service is not buying you anything."
//
// TWO THINGS THIS MEASURES, AND IT IS USELESS WITHOUT BOTH
//
//  1. Throughput and latency. Obvious, and on its own worthless: a limiter that
//     returns "allowed" for everything is extremely fast.
//  2. CORRECTNESS. Every run asserts that the limiter actually limited — allowed
//     counts must match the configured budget per key per window. A rate limiter
//     benchmark that does not verify enforcement is measuring how fast a server
//     can say yes, and would rank a broken implementation first.
//
// Run: see README.md. Output is a single JSON object on stdout so the harness can
// parse it and the raw stdout can be committed as the artifact.
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"sort"
	"sync"
	"sync/atomic"
	"time"

	"github.com/redis/go-redis/v9"
)

type result struct {
	Target          string  `json:"target"`
	Addr            string  `json:"addr"`
	Workers         int     `json:"workers"`
	Keys            int     `json:"keys"`
	BudgetPerWindow int     `json:"budget_per_window"`
	WindowSeconds   int     `json:"window_seconds"`
	DurationSeconds float64 `json:"duration_seconds"`

	Checks      int64   `json:"checks"`
	Allowed     int64   `json:"allowed"`
	Denied      int64   `json:"denied"`
	Errors      int64   `json:"errors"`
	ChecksPerSec float64 `json:"checks_per_sec"`

	P50Micros float64 `json:"p50_micros"`
	P99Micros float64 `json:"p99_micros"`
	MaxMicros float64 `json:"max_micros"`

	// Correctness verdict. A run that fails this is not a slow run, it is an
	// invalid one, and its throughput number must not be used.
	EnforcementOK      bool   `json:"enforcement_ok"`
	EnforcementDetail  string `json:"enforcement_detail"`
	ExpectedMaxAllowed int64  `json:"expected_max_allowed"`
	WindowsTouched     int64  `json:"windows_touched"`
}

func main() {
	addr := flag.String("addr", "127.0.0.1:6379", "RESP server address")
	password := flag.String("password", "", "AUTH password if the server needs one")
	target := flag.String("target", "unknown", "label for this arm, e.g. pg_resp or redis")
	workers := flag.Int("workers", 8, "concurrent limiter clients")
	keys := flag.Int("keys", 1000, "distinct API keys in the flood")
	budget := flag.Int("budget", 100, "allowed requests per key per window")
	window := flag.Int("window", 60, "window length in seconds")
	duration := flag.Duration("duration", 10*time.Second, "flood duration")
	flag.Parse()

	ctx := context.Background()
	// One client per worker: a shared pool would make connection count an
	// uncontrolled variable, and connection count is exactly what the benchmark
	// grid varies deliberately elsewhere.
	clients := make([]*redis.Client, *workers)
	for i := range clients {
		clients[i] = redis.NewClient(&redis.Options{
			Addr:     *addr,
			Password: *password,
			PoolSize: 1,
		})
		if err := clients[i].Ping(ctx).Err(); err != nil {
			fmt.Fprintf(os.Stderr, "cannot reach %s: %v\n", *addr, err)
			os.Exit(1)
		}
	}

	// A unique run id keeps each run's counters separate, so a previous run's
	// leftover state cannot make this run look rate-limited when it is not.
	runID := time.Now().UnixNano()

	var checks, allowed, denied, errs int64
	latencies := make([][]time.Duration, *workers)

	stop := time.Now().Add(*duration)
	var wg sync.WaitGroup
	start := time.Now()
	// Window index at the moment the flood starts. The bucket key is
	// unix/window, so the enforcement ceiling must be computed from these
	// indices rather than from the run's duration.
	startWindow := start.Unix() / int64(*window)

	for w := 0; w < *workers; w++ {
		wg.Add(1)
		go func(w int) {
			defer wg.Done()
			c := clients[w]
			local := make([]time.Duration, 0, 1<<16)
			n := 0
			for time.Now().Before(stop) {
				// Deterministic key rotation rather than a random one: the number
				// of checks per key is then exactly derivable, which is what makes
				// the enforcement assertion below possible at all.
				apiKey := (w + n**workers) % *keys
				n++
				bucket := fmt.Sprintf("rl:%d:%d:%d", runID, apiKey, time.Now().Unix()/int64(*window))

				t0 := time.Now()
				count, err := c.Incr(ctx, bucket).Result()
				if err == nil && count == 1 {
					// First hit in this window: attach the TTL. Only on the first
					// hit, which is the point of the INCR-then-EXPIRE pattern —
					// re-EXPIREing every request would silently make the window
					// sliding and the limiter wrong.
					err = c.Expire(ctx, bucket, time.Duration(*window)*time.Second).Err()
				}
				local = append(local, time.Since(t0))

				atomic.AddInt64(&checks, 1)
				switch {
				case err != nil:
					atomic.AddInt64(&errs, 1)
				case count <= int64(*budget):
					atomic.AddInt64(&allowed, 1)
				default:
					atomic.AddInt64(&denied, 1)
				}
			}
			latencies[w] = local
		}(w)
	}
	wg.Wait()
	elapsed := time.Since(start)
	endWindow := time.Now().Unix() / int64(*window)

	all := make([]time.Duration, 0, int(checks))
	for _, l := range latencies {
		all = append(all, l...)
	}
	sort.Slice(all, func(i, j int) bool { return all[i] < all[j] })

	pct := func(p float64) float64 {
		if len(all) == 0 {
			return 0
		}
		i := int(p * float64(len(all)-1))
		return float64(all[i].Microseconds())
	}

	r := result{
		Target: *target, Addr: *addr, Workers: *workers, Keys: *keys,
		BudgetPerWindow: *budget, WindowSeconds: *window,
		DurationSeconds: elapsed.Seconds(),
		Checks:          checks, Allowed: allowed, Denied: denied, Errors: errs,
		ChecksPerSec: float64(checks) / elapsed.Seconds(),
		P50Micros:    pct(0.50), P99Micros: pct(0.99),
		MaxMicros: func() float64 {
			if len(all) == 0 {
				return 0
			}
			return float64(all[len(all)-1].Microseconds())
		}(),
	}

	// ENFORCEMENT CHECK.
	//
	// The ceiling is budget x keys x (windows touched). Windows touched is derived
	// from the actual wall-clock window indices the run spanned, NOT from its
	// duration.
	//
	// Deriving it from duration is wrong and was caught by the first smoke run: a
	// 3-second flood with a 60-second window computed elapsed/window+1 = 1 window,
	// but the run straddled a real minute boundary, so two buckets each correctly
	// allowed one request. allowed=2 against a ceiling of 1 was reported as "the
	// limiter DID NOT LIMIT" when the limiter was working perfectly. The bucket key
	// is a function of the clock, so the bound has to be too.
	windowsTouched := endWindow - startWindow + 1
	r.ExpectedMaxAllowed = int64(*budget) * int64(*keys) * windowsTouched
	r.WindowsTouched = windowsTouched
	switch {
	case errs > 0:
		r.EnforcementOK = false
		r.EnforcementDetail = fmt.Sprintf(
			"%d errors during the run — throughput from a run with errors is not usable", errs)
	case allowed > r.ExpectedMaxAllowed:
		r.EnforcementOK = false
		r.EnforcementDetail = fmt.Sprintf(
			"limiter DID NOT LIMIT: allowed %d > ceiling %d (budget %d x keys %d x windows %d). "+
				"The throughput number from this run is invalid.",
			allowed, r.ExpectedMaxAllowed, *budget, *keys, windowsTouched)
	case denied == 0 && checks > r.ExpectedMaxAllowed:
		r.EnforcementOK = false
		r.EnforcementDetail = "no request was ever denied although the budget was exceeded — limiter not enforcing"
	default:
		r.EnforcementOK = true
		r.EnforcementDetail = fmt.Sprintf(
			"enforced: %d allowed <= ceiling %d, %d denied", allowed, r.ExpectedMaxAllowed, denied)
	}

	out, _ := json.MarshalIndent(r, "", "  ")
	fmt.Println(string(out))
	if !r.EnforcementOK {
		fmt.Fprintln(os.Stderr, "ENFORCEMENT FAILED: "+r.EnforcementDetail)
		os.Exit(2)
	}
}
