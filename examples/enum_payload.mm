// =============================================================
// Enum Payload Demo: Tagged Unions with Data
// =============================================================
// Plan 14 で導入された enum payload のデモ。
// Shape 型を定義し、match でバリアントごとに面積を計算する。
//
// Usage:
//   mumei check examples/enum_payload.mm

// --- Enum: 図形の種類 ---
// Circle(radius), Rectangle(width, height)
enum Shape {
    Circle(i64),
    Rectangle(i64, i64)
}

// --- 面積計算: match で Circle/Rectangle を分岐 ---
// Circle: radius * radius (π は整数近似)
// Rectangle: width * height
atom area(s: Shape)
    requires: true;
    ensures: result >= 0;
    body: {
        match s {
            Circle(r) => r * r * 3,
            Rectangle(w, h) => w * h
        }
    }

// --- 周囲長計算 ---
atom perimeter(s: Shape)
    requires: true;
    ensures: result >= 0;
    body: {
        match s {
            Circle(r) => r * 6,
            Rectangle(w, h) => 2 * (w + h)
        }
    }
