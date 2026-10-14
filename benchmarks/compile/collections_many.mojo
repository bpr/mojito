# Many instances of the bundled collections: generic `def`s drive `List`,
# `Dict`, and `Set` through most of their surface at a dozen element types,
# and `main` adds nested `List`/`Dict` values and `Optional` at four types.
# Most of the concrete program is std collection instances.
from std.collections import Set


def exercise_list[
    T: Copyable & Movable & Writable & Equatable & Deinitable
](a: T, b: T) raises -> Int:
    var xs = List[T]()
    xs.append(a.copy())
    xs.append(b.copy())
    xs.insert(0, b.copy())
    xs.extend(xs.copy())
    var popped = xs.pop()
    xs.reverse()
    var n = len(xs)
    var found = xs.index(a)
    var has = a in xs
    var m = len(xs[1:3])
    xs.clear()
    print(popped, n, found, has, m, Bool(xs))
    return n


def exercise_dict[
    K: Hashable & Equatable & Copyable & Movable & Deinitable & Writable,
    V: Copyable & Movable & Writable & Deinitable,
](k: K, k2: K, v: V) raises -> Int:
    var d = Dict[K, V]()
    d[k.copy()] = v.copy()
    d[k2.copy()] = v.copy()
    var got = d.get(k)
    var popped = d.pop(k2)
    var dflt = d.setdefault(k2.copy(), v.copy()).copy()
    var n = 0
    for key in d.keys():
        n += 1
        print(key)
    print(got.value(), popped, dflt, len(d), k in d)
    return n


def exercise_set[
    T: Hashable & Equatable & Copyable & Movable & Deinitable & Writable
](a: T, b: T) -> Int:
    var s = Set[T]()
    s.add(a.copy())
    s.add(b.copy())
    var t = Set[T]()
    t.add(a.copy())
    var u = s | t
    var i = s & t
    var d = s - t
    print(len(s), len(u), len(i), len(d), a in s)
    return len(s)


def exercise_optional[
    T: Copyable & Movable & Writable & Deinitable
](x: T) -> Int:
    var some = Optional[T](x.copy())
    var none = Optional[T]()
    print(some.value(), Bool(some), Bool(none))
    return 1 if some else 0


def main() raises:
    var total = 0
    total += exercise_list(1, 2)
    total += exercise_list(Int8(1), Int8(2))
    total += exercise_list(Int16(1), Int16(2))
    total += exercise_list(Int32(1), Int32(2))
    total += exercise_list(Int64(1), Int64(2))
    total += exercise_list(UInt8(1), UInt8(2))
    total += exercise_list(UInt16(1), UInt16(2))
    total += exercise_list(UInt32(1), UInt32(2))
    total += exercise_list(UInt64(1), UInt64(2))
    total += exercise_list(Float32(1.5), Float32(2.5))
    total += exercise_list(Float64(1.5), Float64(2.5))
    total += exercise_list(True, False)
    total += exercise_list(String("a"), String("b"))
    total += exercise_dict(1, 2, String("v"))
    total += exercise_dict(String("a"), String("b"), 3)
    total += exercise_dict(String("a"), String("b"), 2.5)
    total += exercise_dict(UInt8(1), UInt8(2), True)
    total += exercise_dict(Int64(1), Int64(2), Int64(9))
    total += exercise_set(1, 2)
    total += exercise_set(String("a"), String("b"))
    total += exercise_set(UInt16(1), UInt16(2))
    total += exercise_set(Int32(1), Int32(2))
    total += exercise_optional(7)
    total += exercise_optional(String("s"))
    total += exercise_optional(Float64(0.25))
    total += exercise_optional(UInt32(9))

    var grid = List[List[Int]]()
    for r in range(3):
        var row = List[Int]()
        for c in range(4):
            row.append(r * c)
        grid.append(row^)
    var index = Dict[String, List[Int]]()
    index[String("diag")] = [grid[1][1], grid[2][2]]
    index[String("edge")] = grid[2].copy()
    for e in index.items():
        print(e.key, len(e.value))
    print(len(grid), grid[2][3], index[String("diag")][1])
    print(total)
