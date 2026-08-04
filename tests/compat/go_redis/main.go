// go-redis compat check against pg_resp's T0 command set.
//
// NOT LOCALLY VERIFIED: this WSL2 environment has no `go` toolchain
// installed and none could be added without going outside this run's
// allowed write paths (see reports/phase1.md). Written carefully against
// go-redis v9's documented API; run for real via the dockerized compat
// matrix (`docker compose run go-redis`) or any machine with Go.
//
// Usage: go run main.go [host] [port]
// Exit code 0 = all checks passed, 1 = at least one failed.
package main

import (
	"context"
	"fmt"
	"os"

	"github.com/redis/go-redis/v9"
)

type check struct {
	desc     string
	ok       bool
	actual   interface{}
	expected interface{}
}

func main() {
	host := "127.0.0.1"
	port := "6379"
	if len(os.Args) > 1 {
		host = os.Args[1]
	}
	if len(os.Args) > 2 {
		port = os.Args[2]
	}

	ctx := context.Background()
	rdb := redis.NewClient(&redis.Options{Addr: host + ":" + port})
	defer rdb.Close()

	var results []check
	add := func(desc string, actual, expected interface{}, err error) {
		ok := err == nil && fmt.Sprintf("%v", actual) == fmt.Sprintf("%v", expected)
		results = append(results, check{desc, ok, actual, expected})
	}
	addErrOnly := func(desc string, expectErr bool, err error) {
		ok := (err != nil) == expectErr
		results = append(results, check{desc, ok, fmt.Sprintf("err=%v", err), fmt.Sprintf("expectErr=%v", expectErr)})
	}

	pingRes, pingErr := rdb.Ping(ctx).Result()
	add("ping", pingRes, "PONG", pingErr)

	setRes, setErr := rdb.Set(ctx, "k", "v", 0).Result()
	add("set", setRes, "OK", setErr)

	getRes, getErr := rdb.Get(ctx, "k").Result()
	add("get", getRes, "v", getErr)

	_, getMissingErr := rdb.Get(ctx, "missing").Result()
	addErrOnly("get missing is redis.Nil", true, getMissingErr)
	results[len(results)-1].ok = getMissingErr == redis.Nil

	nxRes := rdb.SetNX(ctx, "k", "v2", 0)
	nxOk, nxErr := nxRes.Result()
	add("set nx on existing key returns false", nxOk, false, nxErr)

	delRes, delErr := rdb.Del(ctx, "k").Result()
	add("del", delRes, int64(1), delErr)

	existsRes, existsErr := rdb.Exists(ctx, "k").Result()
	add("exists after del", existsRes, int64(0), existsErr)

	_, msetErr := rdb.MSet(ctx, "a", "1", "b", "2").Result()
	mgetRes, mgetErr := rdb.MGet(ctx, "a", "z", "b").Result()
	mgetOk := msetErr == nil && mgetErr == nil &&
		fmt.Sprintf("%v", mgetRes) == fmt.Sprintf("%v", []interface{}{"1", nil, "2"})
	results = append(results, check{"mset/mget", mgetOk, mgetRes, []interface{}{"1", nil, "2"}})

	incrRes, incrErr := rdb.Incr(ctx, "ctr").Result()
	add("incr", incrRes, int64(1), incrErr)
	incrRes2, incrErr2 := rdb.Incr(ctx, "ctr").Result()
	add("incr again", incrRes2, int64(2), incrErr2)

	rdb.Set(ctx, "ek", "v", 0)
	expireRes, expireErr := rdb.Expire(ctx, "ek", 10).Result()
	add("expire", expireRes, true, expireErr)
	ttlRes, ttlErr := rdb.TTL(ctx, "ek").Result()
	add("ttl", ttlRes.Seconds(), float64(10), ttlErr)

	failures := 0
	for _, c := range results {
		status := "OK"
		if !c.ok {
			status = "FAIL"
			failures++
		}
		fmt.Printf("%s %s: got=%v expected=%v\n", status, c.desc, c.actual, c.expected)
	}
	fmt.Printf("\n%d/%d passed\n", len(results)-failures, len(results))
	if failures > 0 {
		os.Exit(1)
	}
}
