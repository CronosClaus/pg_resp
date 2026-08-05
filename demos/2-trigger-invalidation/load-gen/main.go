package main

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"math"
	"math/rand"
	"net/http"
	"sync"
	"sync/atomic"
	"time"

	"gorm.io/driver/postgres"
	"gorm.io/gorm"
)

type Product struct {
	ID    int    `gorm:"primaryKey"`
	Name  string
	Price string
}

type Stats struct {
	TotalReads        int64
	TotalWrites       int64
	WritesPrimary     int64
	WritesBulk        int64
	StaleServes       int64
	MaxStaleDuration  time.Duration
	StaleDurations    []time.Duration
	mu                sync.Mutex
	allStaleDurations []time.Duration
}

type LoadGen struct {
	appURL string
	dbConn *gorm.DB
	stats  *Stats
}

var (
	lg *LoadGen
)

func init() {
	// Connect to Postgres (same as app, for truth reading)
	dsn := "host=pg_resp user=demo_user password=demo_pass dbname=postgres port=5432 sslmode=disable"
	db, err := gorm.Open(postgres.Open(dsn), &gorm.Config{})
	if err != nil {
		log.Fatalf("Failed to connect to database: %v", err)
	}

	lg = &LoadGen{
		appURL: "http://app:8080",
		dbConn: db,
		stats:  &Stats{},
	}

	// Wait for app to be ready
	for i := 0; i < 30; i++ {
		resp, err := http.Get(lg.appURL + "/health")
		if err == nil && resp.StatusCode == http.StatusOK {
			resp.Body.Close()
			break
		}
		if err == nil {
			resp.Body.Close()
		}
		log.Printf("Waiting for app... (%d/30)", i+1)
		time.Sleep(1 * time.Second)
	}

	// Create test products
	numProducts := 10
	for i := 1; i <= numProducts; i++ {
		lg.dbConn.Exec("DELETE FROM products WHERE id = ?", i)
		if err := lg.dbConn.Create(&Product{
			ID:    i,
			Name:  fmt.Sprintf("Product %d", i),
			Price: fmt.Sprintf("%d.00", 10+i),
		}).Error; err != nil {
			log.Fatalf("Failed to create product: %v", err)
		}
	}
	log.Printf("Created %d test products", numProducts)
}

func main() {
	log.Printf("Starting load generator")

	// Run for a fixed duration
	duration := 30 * time.Second
	ctx, cancel := context.WithTimeout(context.Background(), duration)
	defer cancel()

	// Goroutines: writers + readers
	numWriters := 4
	numReaders := 8

	var wg sync.WaitGroup

	// Start writers
	for i := 0; i < numWriters; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			writeLoop(ctx, id)
		}(i)
	}

	// Start readers
	for i := 0; i < numReaders; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			readLoop(ctx, id)
		}(i)
	}

	// Wait for all goroutines
	wg.Wait()

	// Print results
	printResults()
}

func writeLoop(ctx context.Context, id int) {
	for {
		select {
		case <-ctx.Done():
			return
		default:
		}

		productID := rand.Intn(10) + 1
		newPrice := fmt.Sprintf("%.2f", 10+float64(rand.Intn(90)))

		// Randomly choose between primary and bulk paths
		if rand.Float64() < 0.5 {
			// Primary path: PUT /products/{id}
			updateViaAPI(productID, newPrice)
			atomic.AddInt64(&lg.stats.WritesPrimary, 1)
		} else {
			// Bulk path: POST /products/{id}/bulk-reprice (the buggy path in ARM A)
			bulkRepriceViaAPI(productID, 10.0)
			atomic.AddInt64(&lg.stats.WritesBulk, 1)
		}

		atomic.AddInt64(&lg.stats.TotalWrites, 1)
		time.Sleep(10 * time.Millisecond) // Spacing between writes
	}
}

func readLoop(ctx context.Context, id int) {
	for {
		select {
		case <-ctx.Done():
			return
		default:
		}

		productID := rand.Intn(10) + 1
		atomic.AddInt64(&lg.stats.TotalReads, 1)

		// Read from app (may be stale)
		cachedPrice := readFromApp(productID)
		if cachedPrice == "" {
			continue
		}

		// Read truth from DB
		truth := readFromDB(productID)

		// Check for staleness
		if cachedPrice != truth {
			atomic.AddInt64(&lg.stats.StaleServes, 1)

			// Record staleness duration (estimate: poll-based)
			duration := measureStaleDuration(productID, cachedPrice)
			lg.stats.mu.Lock()
			lg.stats.allStaleDurations = append(lg.stats.allStaleDurations, duration)
			if duration > lg.stats.MaxStaleDuration {
				lg.stats.MaxStaleDuration = duration
			}
			lg.stats.mu.Unlock()
		}

		time.Sleep(5 * time.Millisecond)
	}
}

var httpClient = &http.Client{Timeout: 5 * time.Second}

func updateViaAPI(productID int, newPrice string) {
	body, _ := json.Marshal(map[string]string{"price": newPrice})
	resp, err := httpClient.Post(
		fmt.Sprintf("%s/products/%d", lg.appURL, productID),
		"application/json",
		bytes.NewReader(body),
	)
	if err == nil {
		io.ReadAll(resp.Body)
		resp.Body.Close()
	}
}

func bulkRepriceViaAPI(productID int, discountPercent float64) {
	body, _ := json.Marshal(map[string]float64{"discount_percent": discountPercent})
	resp, err := httpClient.Post(
		fmt.Sprintf("%s/products/%d/bulk-reprice", lg.appURL, productID),
		"application/json",
		bytes.NewReader(body),
	)
	if err == nil {
		io.ReadAll(resp.Body)
		resp.Body.Close()
	}
}

func readFromApp(productID int) string {
	resp, err := httpClient.Get(fmt.Sprintf("%s/products/%d", lg.appURL, productID))
	if err != nil {
		return ""
	}
	defer resp.Body.Close()

	var product Product
	json.NewDecoder(resp.Body).Decode(&product)
	return product.Price
}

func readFromDB(productID int) string {
	var product Product
	if err := lg.dbConn.First(&product, productID).Error; err != nil {
		return ""
	}
	return product.Price
}

// measureStaleDuration: polls the app until it matches the DB truth, measuring how long.
// This is a rough measure; the actual window may be shorter due to polling granularity.
func measureStaleDuration(productID int, staledPrice string) time.Duration {
	start := time.Now()
	for {
		if time.Since(start) > 10*time.Second {
			// Gave up waiting; assume max duration
			return 10 * time.Second
		}
		cached := readFromApp(productID)
		truth := readFromDB(productID)
		if cached == truth && cached != staledPrice {
			// Convergence detected
			return time.Since(start)
		}
		time.Sleep(1 * time.Millisecond)
	}
}

func printResults() {
	fmt.Println("\n=== Load Generation Results ===\n")

	totalReads := atomic.LoadInt64(&lg.stats.TotalReads)
	totalWrites := atomic.LoadInt64(&lg.stats.TotalWrites)
	writesPrimary := atomic.LoadInt64(&lg.stats.WritesPrimary)
	writesBulk := atomic.LoadInt64(&lg.stats.WritesBulk)
	staleServes := atomic.LoadInt64(&lg.stats.StaleServes)

	fmt.Printf("Total reads:       %d\n", totalReads)
	fmt.Printf("Total writes:      %d\n", totalWrites)
	fmt.Printf("  Primary path:    %d\n", writesPrimary)
	fmt.Printf("  Bulk path:       %d\n", writesBulk)
	fmt.Printf("Stale serves:      %d (%.2f%%)\n", staleServes,
		float64(staleServes)*100/float64(totalReads))
	fmt.Printf("Max staleness:     %v\n", lg.stats.MaxStaleDuration)

	if len(lg.stats.allStaleDurations) > 0 {
		durations := lg.stats.allStaleDurations
		pcts := percentiles(durations, 0.5, 0.99)
		if len(pcts) >= 2 {
			fmt.Printf("Staleness p50:     %v\n", pcts[0])
			fmt.Printf("Staleness p99:     %v\n", pcts[1])
		}
	}

	fmt.Println()
}

func percentiles(durations []time.Duration, ps ...float64) []time.Duration {
	if len(durations) == 0 {
		return make([]time.Duration, len(ps))
	}
	// Convert to float64 for sorting and percentile calc
	times := make([]float64, len(durations))
	for i, d := range durations {
		times[i] = float64(d.Nanoseconds())
	}
	// Simple percentile: sort and index
	// For proper percentile, we'd use interpolation, but this is good enough
	for i := 0; i < len(times); i++ {
		for j := i + 1; j < len(times); j++ {
			if times[i] > times[j] {
				times[i], times[j] = times[j], times[i]
			}
		}
	}

	result := make([]time.Duration, len(ps))
	for i, p := range ps {
		idx := int(math.Ceil(float64(len(times)) * p))
		if idx >= len(times) {
			idx = len(times) - 1
		}
		if idx < 0 {
			idx = 0
		}
		result[i] = time.Duration(int64(times[idx]))
	}
	return result
}
