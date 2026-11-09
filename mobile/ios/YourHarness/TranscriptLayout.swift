// The transcript's layout: rows stacked by heights measured beforehand, so laying out is a
// running sum and finding the rows on screen a binary search, however many rows are held.

import UIKit

final class TranscriptLayout: UICollectionViewLayout {
    static let insets = UIEdgeInsets(top: 12, left: 16, bottom: 24, right: 16)

    /// One per item, in order; set before the collection view learns of a change.
    var heights: [CGFloat] = [] {
        didSet { tops = [] }
    }
    private var tops: [CGFloat] = []
    private var total: CGFloat = 0

    /// The width rows are measured for.
    static func contentWidth(for width: CGFloat) -> CGFloat {
        width - insets.left - insets.right
    }

    override func prepare() {
        guard tops.count != heights.count else { return }
        tops = []
        tops.reserveCapacity(heights.count)
        var y: CGFloat = 0
        for h in heights {
            tops.append(y)
            y += h
        }
        total = y
    }

    override var collectionViewContentSize: CGSize {
        CGSize(width: collectionView?.bounds.width ?? 0, height: total + Self.insets.top + Self.insets.bottom)
    }

    private func frame(_ i: Int) -> CGRect {
        let width = collectionView?.bounds.width ?? 0
        return CGRect(x: Self.insets.left, y: Self.insets.top + tops[i], width: Self.contentWidth(for: width), height: heights[i])
    }

    override func layoutAttributesForElements(in rect: CGRect) -> [UICollectionViewLayoutAttributes]? {
        guard !tops.isEmpty else { return [] }
        // The first row reaching into `rect`.
        var lo = 0, hi = tops.count - 1
        let top = rect.minY - Self.insets.top
        while lo < hi {
            let mid = (lo + hi + 1) / 2
            if tops[mid] <= top { lo = mid } else { hi = mid - 1 }
        }
        var out: [UICollectionViewLayoutAttributes] = []
        var i = lo
        while i < tops.count, Self.insets.top + tops[i] < rect.maxY {
            let a = UICollectionViewLayoutAttributes(forCellWith: IndexPath(item: i, section: 0))
            a.frame = frame(i)
            out.append(a)
            i += 1
        }
        return out
    }

    override func layoutAttributesForItem(at indexPath: IndexPath) -> UICollectionViewLayoutAttributes? {
        guard indexPath.item < tops.count else { return nil }
        let a = UICollectionViewLayoutAttributes(forCellWith: indexPath)
        a.frame = frame(indexPath.item)
        return a
    }

    override func shouldInvalidateLayout(forBoundsChange newBounds: CGRect) -> Bool {
        newBounds.width != collectionView?.bounds.width
    }
}
