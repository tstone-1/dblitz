"""Build the synthetic demo database used for the README screenshots.

Usage:  uv run scripts/make-demo-db.py [--replace] [out.sqlite]   (default: demo/roastery.sqlite)

Everything is invented: a fictional coffee roaster's catalogue, customers and
orders, generated from a fixed seed so every run writes the same rows. No real
person, company or record is represented.
"""

import argparse
import os
import random
import sqlite3
from contextlib import closing
from datetime import datetime, timedelta
from pathlib import Path
from tempfile import TemporaryDirectory

SEED = 26_10_29

FIRST = ["Ada", "Ben", "Clara", "Dario", "Elif", "Finn", "Greta", "Hugo", "Ines", "Jonas",
         "Kaito", "Lena", "Mateo", "Nora", "Oskar", "Priya", "Quinn", "Rosa", "Sami", "Tilda",
         "Umar", "Vera", "Wim", "Yara", "Zoe"]
LAST = ["Abara", "Brandt", "Castell", "Dorsey", "Eklund", "Fontaine", "Garrido", "Halvorsen",
        "Ivanova", "Jansen", "Kowalski", "Lindqvist", "Moreau", "Novak", "Okafor", "Petrov",
        "Quist", "Rinaldi", "Sato", "Tanaka", "Ueda", "Varga", "Weber", "Yilmaz", "Zeller"]
CITIES = [("Amsterdam", "NL"), ("Antwerp", "BE"), ("Berlin", "DE"), ("Copenhagen", "DK"),
          ("Dublin", "IE"), ("Hamburg", "DE"), ("Lisbon", "PT"), ("Lyon", "FR"),
          ("Milan", "IT"), ("Oslo", "NO"), ("Rotterdam", "NL"), ("Stockholm", "SE"),
          ("Utrecht", "NL"), ("Vienna", "AT"), ("Zurich", "CH")]
ORIGINS = [("Ethiopia", "Yirgacheffe"), ("Ethiopia", "Guji"), ("Kenya", "Nyeri"),
           ("Colombia", "Huila"), ("Colombia", "Nariño"), ("Brazil", "Cerrado"),
           ("Guatemala", "Huehuetenango"), ("Costa Rica", "Tarrazú"), ("Peru", "Cajamarca"),
           ("Rwanda", "Nyamasheke"), ("Indonesia", "Aceh"), ("Honduras", "Marcala")]
PROCESSES = ["washed", "natural", "honey", "anaerobic"]
ROASTS = ["light", "medium-light", "medium", "dark"]
NOTES = ["jasmine, bergamot", "blueberry, cocoa", "blackcurrant, grapefruit", "caramel, red apple",
         "milk chocolate, hazelnut", "brown sugar, plum", "peach, black tea", "cedar, dark chocolate",
         "cherry, panela", "orange zest, honey"]
STATUSES = ["delivered"] * 14 + ["shipped"] * 3 + ["processing", "cancelled"]


def build(con: sqlite3.Connection, rng: random.Random) -> None:
    con.executescript("""
    CREATE TABLE products (
      id INTEGER PRIMARY KEY,
      name TEXT NOT NULL,
      country TEXT NOT NULL,
      region TEXT NOT NULL,
      process TEXT NOT NULL,
      roast TEXT NOT NULL,
      tasting_notes TEXT,
      price_eur REAL NOT NULL,
      bag_grams INTEGER NOT NULL,
      in_stock INTEGER NOT NULL
    );
    CREATE TABLE customers (
      id INTEGER PRIMARY KEY,
      name TEXT NOT NULL,
      email TEXT NOT NULL,
      city TEXT NOT NULL,
      country TEXT NOT NULL,
      joined DATE NOT NULL,
      newsletter INTEGER NOT NULL
    );
    CREATE TABLE orders (
      id INTEGER PRIMARY KEY,
      customer_id INTEGER NOT NULL REFERENCES customers(id),
      ordered_at TEXT NOT NULL,
      status TEXT NOT NULL,
      shipping_eur REAL NOT NULL,
      total_eur REAL NOT NULL
    );
    CREATE TABLE order_items (
      order_id INTEGER NOT NULL REFERENCES orders(id),
      product_id INTEGER NOT NULL REFERENCES products(id),
      qty INTEGER NOT NULL,
      unit_price_eur REAL NOT NULL,
      PRIMARY KEY (order_id, product_id)
    ) WITHOUT ROWID;
    CREATE INDEX orders_customer ON orders(customer_id);
    CREATE INDEX orders_date ON orders(ordered_at);
    CREATE VIEW monthly_revenue AS
      SELECT substr(ordered_at, 1, 7) AS month,
             count(*) AS orders,
             round(sum(total_eur), 2) AS revenue_eur
      FROM orders WHERE status <> 'cancelled'
      GROUP BY month ORDER BY month;
    """)

    products = []
    for i, (country, region) in enumerate(ORIGINS * 2, start=1):
        process = rng.choice(PROCESSES)
        price = round(rng.uniform(9.5, 24.0) * 2) / 2
        grams = rng.choice([250, 250, 500, 1000])
        name = f"{region} {process.capitalize()}" + (" Reserve" if i > len(ORIGINS) else "")
        products.append((i, name, country, region, process, rng.choice(ROASTS),
                         rng.choice(NOTES), price * grams / 250, grams, rng.randint(0, 340)))
    con.executemany("INSERT INTO products VALUES (?,?,?,?,?,?,?,?,?,?)", products)

    start = datetime(2024, 1, 1)
    customers = []
    for i in range(1, 2501):
        first, last = rng.choice(FIRST), rng.choice(LAST)
        city, country = rng.choice(CITIES)
        joined = start + timedelta(days=rng.randint(0, 900))
        customers.append((i, f"{first} {last}", f"{first}.{last}{i}@example.com".lower(),
                          city, country, joined.date().isoformat(), int(rng.random() < 0.4)))
    con.executemany("INSERT INTO customers VALUES (?,?,?,?,?,?,?)", customers)

    orders, items = [], []
    for oid in range(1, 60001):
        cid = rng.randint(1, len(customers))
        when = start + timedelta(minutes=rng.randint(0, 1000 * 24 * 60))
        lines = rng.sample(products, k=rng.choice([1, 1, 1, 2, 2, 3]))
        total = 0.0
        for p in lines:
            qty = rng.choice([1, 1, 1, 2, 3])
            items.append((oid, p[0], qty, p[7]))
            total += qty * p[7]
        shipping = 0.0 if total >= 40 else 4.9
        orders.append((oid, cid, when.isoformat(sep=" ", timespec="minutes"),
                       rng.choice(STATUSES), shipping, round(total + shipping, 2)))
    con.executemany("INSERT INTO orders VALUES (?,?,?,?,?,?)", orders)
    con.executemany("INSERT INTO order_items VALUES (?,?,?,?)", items)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("output", nargs="?", type=Path, default=Path("demo/roastery.sqlite"))
    parser.add_argument("--replace", action="store_true", help="replace an existing demo file")
    args = parser.parse_args()
    out = args.output
    out.parent.mkdir(parents=True, exist_ok=True)
    if out.exists() and not args.replace:
        parser.exit(1, f"File exists: {out}. Use --replace to regenerate it.\n")

    # Build beside the destination so a failed generation leaves existing data intact.
    with TemporaryDirectory(dir=out.parent) as staging:
        generated = Path(staging) / "roastery.sqlite"
        with closing(sqlite3.connect(generated)) as con:
            with con:
                build(con, random.Random(SEED))
            con.execute("VACUUM")
        if args.replace:
            os.replace(generated, out)
        else:
            # Another process can create the destination while we build.
            os.link(generated, out)
    print(f"[OK] wrote {out} ({out.stat().st_size // 1024} KiB)")


if __name__ == "__main__":
    main()
