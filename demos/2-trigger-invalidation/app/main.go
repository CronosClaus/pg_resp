package main

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/redis/go-redis/v9"
	"gorm.io/driver/postgres"
	"gorm.io/gorm"
)

type Product struct {
	ID    int    `json:"id" gorm:"primaryKey"`
	Name  string `json:"name"`
	Price string `json:"price"` // numeric stored as string for simplicity
}

type App struct {
	db    *gorm.DB
	cache *redis.Client
	mode  string // "app" or "trigger"
}

var (
	dbConn   *gorm.DB
	cacheConn *redis.Client
	app      *App
)

func init() {
	invalidation := os.Getenv("INVALIDATION")
	if invalidation == "" {
		invalidation = "trigger"
	}

	// Connect to Postgres with retries
	dsn := "host=pg_resp user=demo_user password=demo_pass dbname=postgres port=5432 sslmode=disable"
	var db *gorm.DB
	var err error
	for i := 0; i < 30; i++ {
		db, err = gorm.Open(postgres.Open(dsn), &gorm.Config{})
		if err == nil {
			break
		}
		log.Printf("Waiting for database... (%d/30): %v", i+1, err)
		time.Sleep(1 * time.Second)
	}
	if err != nil {
		log.Fatalf("Failed to connect to database: %v", err)
	}
	dbConn = db

	// Connect to Redis (pg_resp)
	cacheConn = redis.NewClient(&redis.Options{
		Addr:     "pg_resp:6379",
		Password: "",
		DB:       0,
	})
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if err := cacheConn.Ping(ctx).Err(); err != nil {
		log.Fatalf("Failed to connect to cache: %v", err)
	}

	app = &App{
		db:    dbConn,
		cache: cacheConn,
		mode:  invalidation,
	}

	// Auto-migrate the schema
	dbConn.AutoMigrate(&Product{})
}

func main() {
	http.HandleFunc("/products", handleListProducts)
	http.HandleFunc("/products/", handleProductOps)
	http.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		fmt.Fprintf(w, "ok")
	})

	log.Printf("Starting server (INVALIDATION=%s) on :8080", app.mode)
	if err := http.ListenAndServe(":8080", nil); err != nil {
		log.Fatalf("Server error: %v", err)
	}
}

func handleListProducts(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}
	var products []Product
	if err := app.db.Find(&products).Error; err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(products)
}

func handleProductOps(w http.ResponseWriter, r *http.Request) {
	parts := strings.Split(r.URL.Path, "/")
	if len(parts) < 3 {
		http.Error(w, "Invalid path", http.StatusBadRequest)
		return
	}

	idStr := parts[2]
	id, err := strconv.Atoi(idStr)
	if err != nil {
		http.Error(w, "Invalid product ID", http.StatusBadRequest)
		return
	}

	// Check for subpath operations
	if len(parts) > 3 && parts[3] == "bulk-reprice" {
		if r.Method != http.MethodPost {
			http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
			return
		}
		handleBulkReprice(w, r, id)
		return
	}

	switch r.Method {
	case http.MethodGet:
		handleGetProduct(w, r, id)
	case http.MethodPut:
		handleUpdateProduct(w, r, id)
	default:
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
	}
}

func handleGetProduct(w http.ResponseWriter, r *http.Request, id int) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	cacheKey := fmt.Sprintf("product:%d", id)

	// Try cache first
	cachedPrice, err := app.cache.Get(ctx, cacheKey).Result()
	if err == nil {
		// Cache hit
		var product Product
		if err := app.db.First(&product, id).Error; err != nil {
			http.Error(w, "Product not found", http.StatusNotFound)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		w.Header().Set("X-Cache", "HIT")
		// Return the cached price (this may be stale!)
		product.Price = cachedPrice
		json.NewEncoder(w).Encode(product)
		return
	}

	// Cache miss, fetch from DB
	var product Product
	if err := app.db.First(&product, id).Error; err != nil {
		http.Error(w, "Product not found", http.StatusNotFound)
		return
	}

	// Populate cache for next time
	app.cache.Set(ctx, cacheKey, product.Price, 0)

	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("X-Cache", "MISS")
	json.NewEncoder(w).Encode(product)
}

func handleUpdateProduct(w http.ResponseWriter, r *http.Request, id int) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	var req struct {
		Price string `json:"price"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "Invalid request", http.StatusBadRequest)
		return
	}

	// Update in DB
	if err := app.db.Model(&Product{}).Where("id = ?", id).Update("price", req.Price).Error; err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	// ARM A only: the application remembers to invalidate here. ARM B has NO
	// invalidation code anywhere in this file — the trigger on the products
	// table does it, which is the whole claim the demo exists to test.
	cacheKey := fmt.Sprintf("product:%d", id)
	if app.mode == "app" {
		app.cache.Del(ctx, cacheKey)
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	fmt.Fprintf(w, `{"status":"updated"}`)
}

// handleBulkReprice: represents a code path that updates the database.
//
// In ARM A (app mode): this is a DELIBERATELY BUGGY path. The primary
// handleUpdateProduct() path calls cache.Del(), but this bulk-reprice endpoint
// does not, simulating a realistic scenario where a new endpoint is added without
// cache integration. This demonstrates that app-side invalidation is fragile and
// error-prone: the responsibility to invalidate is spread across the codebase,
// and the bulk-reprice operation was coded without calling cache.Del().
//
// In ARM B (trigger mode): this path does not invalidate either — and neither
// does any other path, because ARM B's application contains no cache
// invalidation code at all. The AFTER UPDATE trigger created in init-arm-b.sql
// evicts the key at commit. That is the difference being measured: ARM A's
// correctness depends on every author of every write path remembering; ARM B's
// is a property of the schema.
func handleBulkReprice(w http.ResponseWriter, r *http.Request, id int) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	var req struct {
		DiscountPercent float64 `json:"discount_percent"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, "Invalid request", http.StatusBadRequest)
		return
	}

	// Update in DB with discount applied
	if err := app.db.Model(&Product{}).Where("id = ?", id).
		Update("price", gorm.Expr("(CAST(price AS FLOAT) * ?)", 1-req.DiscountPercent/100)).Error; err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	// DELIBERATE BUG (ARM A): no cache invalidation here. This is the realistic
	// failure — a second write path, added later, whose author did not know the
	// cache existed. In ARM B nothing is needed here at all, because the trigger
	// covers every path including this one.
	_ = ctx

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	fmt.Fprintf(w, `{"status":"bulk-repriced"}`)
}


