//! Affine matrices and rectangles, in the PDF row-vector convention.

/// A 2D affine transform `[a b c d e f]`: `x' = a*x + c*y + e`, `y' = b*x + d*y + f`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix(pub [f64; 6]);

impl Matrix {
    pub const IDENTITY: Matrix = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    /// A pure translation.
    pub fn translate(tx: f64, ty: f64) -> Self {
        Matrix([1.0, 0.0, 0.0, 1.0, tx, ty])
    }

    /// A pure scale about the origin.
    pub fn scale(sx: f64, sy: f64) -> Self {
        Matrix([sx, 0.0, 0.0, sy, 0.0, 0.0])
    }

    /// A scale about the fixed point `(cx, cy)`.
    pub fn scale_about(sx: f64, sy: f64, cx: f64, cy: f64) -> Self {
        Matrix([sx, 0.0, 0.0, sy, cx - sx * cx, cy - sy * cy])
    }

    /// The transform that maps rectangle `from` onto rectangle `to`.
    pub fn rect_to_rect(from: &Rect, to: &Rect) -> Option<Self> {
        let (w, h) = (from.width(), from.height());
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let (sx, sy) = (to.width() / w, to.height() / h);
        Some(Matrix([
            sx,
            0.0,
            0.0,
            sy,
            to.x0 - sx * from.x0,
            to.y0 - sy * from.y0,
        ]))
    }

    /// Composition: applies `self` first, then `next`.
    pub fn then(&self, next: &Matrix) -> Matrix {
        let [a, b, c, d, e, f] = self.0;
        let [g, h, i, j, k, l] = next.0;
        Matrix([
            a * g + b * i,
            a * h + b * j,
            c * g + d * i,
            c * h + d * j,
            e * g + f * i + k,
            e * h + f * j + l,
        ])
    }

    /// The inverse transform, or `None` when the matrix is singular or not finite.
    pub fn invert(&self) -> Option<Matrix> {
        let [a, b, c, d, e, f] = self.0;
        let det = a * d - b * c;
        if !det.is_finite() || det.abs() < 1e-12 {
            return None;
        }
        let m = Matrix([
            d / det,
            -b / det,
            -c / det,
            a / det,
            (c * f - d * e) / det,
            (b * e - a * f) / det,
        ]);
        m.is_finite().then_some(m)
    }

    /// Transforms a point.
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + c * y + e, b * x + d * y + f)
    }

    /// Transforms a direction, ignoring the translation.
    pub fn apply_vec(&self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, _, _] = self.0;
        (a * x + c * y, b * x + d * y)
    }

    /// True when every component is a finite number.
    pub fn is_finite(&self) -> bool {
        self.0.iter().all(|v| v.is_finite())
    }

    /// The same linear part with the translation replaced so the origin lands on `(x, y)`.
    pub fn with_origin(&self, x: f64, y: f64) -> Matrix {
        let [a, b, c, d, _, _] = self.0;
        Matrix([a, b, c, d, x, y])
    }
}

/// An axis-aligned rectangle with `x0 <= x1` and `y0 <= y1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect {
    /// Builds a rectangle from two opposite corners given in any order.
    pub fn new(xa: f64, ya: f64, xb: f64, yb: f64) -> Self {
        Rect {
            x0: xa.min(xb),
            y0: ya.min(yb),
            x1: xa.max(xb),
            y1: ya.max(yb),
        }
    }

    /// The bounding box of the unit-free quad `(x0, y0)-(x1, y1)` after `m` is applied.
    pub fn quad_bbox(m: &Matrix, x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        let p = [
            m.apply(x0, y0),
            m.apply(x1, y0),
            m.apply(x1, y1),
            m.apply(x0, y1),
        ];
        let mut r = Rect {
            x0: p[0].0,
            y0: p[0].1,
            x1: p[0].0,
            y1: p[0].1,
        };
        for (x, y) in &p[1..] {
            r.add_point(*x, *y);
        }
        r
    }

    /// Grows the rectangle to contain a point.
    pub fn add_point(&mut self, x: f64, y: f64) {
        self.x0 = self.x0.min(x);
        self.y0 = self.y0.min(y);
        self.x1 = self.x1.max(x);
        self.y1 = self.y1.max(y);
    }

    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }

    /// The smallest rectangle containing both.
    pub fn union(&self, o: &Rect) -> Rect {
        Rect {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }

    /// The overlap of both; a degenerate rectangle when they do not intersect.
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x0 = self.x0.max(o.x0);
        let y0 = self.y0.max(o.y0);
        Rect {
            x0,
            y0,
            x1: self.x1.min(o.x1).max(x0),
            y1: self.y1.min(o.y1).max(y0),
        }
    }

    /// True when `o` lies inside `self`, allowing `tol` points of slack.
    pub fn contains(&self, o: &Rect, tol: f64) -> bool {
        o.x0 >= self.x0 - tol
            && o.y0 >= self.y0 - tol
            && o.x1 <= self.x1 + tol
            && o.y1 <= self.y1 + tol
    }

    /// The bounding box of this rectangle after `m` is applied.
    pub fn transform(&self, m: &Matrix) -> Rect {
        Rect::quad_bbox(m, self.x0, self.y0, self.x1, self.y1)
    }

    /// True when all four coordinates are finite.
    pub fn is_finite(&self) -> bool {
        self.x0.is_finite() && self.y0.is_finite() && self.x1.is_finite() && self.y1.is_finite()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composes_and_inverts() {
        let m = Matrix([2.0, 0.0, 0.0, 3.0, 10.0, 20.0]).then(&Matrix::translate(5.0, -5.0));
        assert_eq!(m.apply(1.0, 1.0), (17.0, 18.0));
        let back = m.then(&m.invert().unwrap());
        for (got, want) in back.0.iter().zip(Matrix::IDENTITY.0) {
            assert!((got - want).abs() < 1e-9);
        }
        assert!(Matrix([1.0, 2.0, 2.0, 4.0, 0.0, 0.0]).invert().is_none());
    }

    #[test]
    fn maps_rect_to_rect() {
        let from = Rect::new(10.0, 10.0, 20.0, 30.0);
        let to = Rect::new(0.0, 0.0, 5.0, 5.0);
        let m = Matrix::rect_to_rect(&from, &to).unwrap();
        assert_eq!(from.transform(&m), to);
    }
}
