#!/bin/zsh
# A small project with a git repo: code, a 100,000-line file, a picture, a binary and ignored folders.
set -euo pipefail
SHOP="$1/shop"
mkdir -p "$SHOP/src/checkout" "$SHOP/src/data" "$SHOP/src/ui" "$SHOP/docs" "$SHOP/build" "$SHOP/node_modules/left-pad"
cat > "$SHOP/src/checkout/cart.ts" <<'TS'
import { tax } from "./tax";

export interface Item {
  name: string;
  price: number;
  quantity: number;
}

// The cart's total, with tax.
export function total(items: Item[]): number {
  let sum = 0;
  for (const item of items) sum += item.price * item.quantity;
  sum += tax(sum);
  return Math.round(sum * 100) / 100
}
TS
cat > "$SHOP/src/checkout/tax.ts" <<'TS'
/// Tax by the rules in docs/pricing.md.
export function tax(amount: number): number {
  if (amount <= 0) return 0;
  const rate = 0.2;
  // Rounded to cents.
  const owed = amount * rate;
  return Math.round(owed * 100) / 100;
}
TS
printf '# Pricing\n\nPrices are in cents.\n\nTax is 20%%, rounded to the cent.\n' > "$SHOP/docs/pricing.md"
printf 'build/\nnode_modules/\n' > "$SHOP/.gitignore"
printf 'module.exports = (s) => s;\n' > "$SHOP/node_modules/left-pad/index.js"
printf 'compiled\n' > "$SHOP/build/app.js"
python3 -I - "$SHOP/src/data/catalog.json" <<'PY'
import json, sys
with open(sys.argv[1], "w") as f:
    f.write("[\n")
    for i in range(100_000):
        f.write('  {"id": %d, "name": "Item %d", "price": %d, "tags": ["a", "b"]}%s\n' % (i, i, i * 7 % 10000, "," if i < 99_999 else ""))
    f.write("]\n")
PY
# A 64×64 PNG and a few bytes of binary.
python3 -I - "$SHOP/src/ui/Logo.png" "$SHOP/src/data/blob.bin" <<'PY'
import struct, sys, zlib
w = h = 64
rows = b"".join(b"\x00" + b"".join(bytes((x * 4, y * 4, 200)) for x in range(w)) for y in range(h))
def chunk(kind, data): return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")
open(sys.argv[1], "wb").write(png)
open(sys.argv[2], "wb").write(bytes(range(256)) * 4)
PY
git -C "$SHOP" init -q
git -C "$SHOP" add -A
git -C "$SHOP" -c user.name=Preset -c user.email=preset@example.com commit -qm "Shop"
