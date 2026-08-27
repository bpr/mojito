# Set method growth: literal construction, remove/discard/pop (LIFO,
# raising), update, set algebra with operators, comparisons,
# issubset/issuperset/isdisjoint, __bool__, clear — identical output on
# both compilers.
from std.collections.set import Set

def main() raises:
    var s: Set[Int] = {1, 2, 3, 3}
    print(len(s))
    s.remove(2)
    print(len(s), 2 in s)
    s.discard(99)
    s.discard(3)
    print(len(s))
    s.add(4)
    s.add(5)
    print(s.pop())
    var t: Set[Int] = {1, 4, 7}
    s.update(t)
    print(len(s))
    var u = s.union(t)
    var n = s.intersection(t)
    var d = s.difference(t)
    var x = s.symmetric_difference(t)
    print(len(u), len(n), len(d), len(x))
    var a: Set[Int] = {1, 2, 3}
    var b: Set[Int] = {3, 2, 1}
    var c: Set[Int] = {1, 2}
    print(Bool(a == b), Bool(a != b), Bool(a == c))
    print(Bool(c <= a), Bool(c < a), Bool(a <= b), Bool(a < b))
    print(Bool(a >= c), Bool(a > c), Bool(a > b))
    print(Bool(c.issubset(a)), Bool(a.issuperset(c)), Bool(c.isdisjoint(t)))
    var ops_and = a & b
    var ops_or = a | c
    var ops_sub = a - c
    var ops_xor = a ^ c
    print(len(ops_and), len(ops_or), len(ops_sub), len(ops_xor))
    a -= c
    print(len(a), 3 in a)
    a.intersection_update(b)
    print(len(a))
    b.difference_update(c)
    print(len(b), 3 in b)
    b.symmetric_difference_update(c)
    print(len(b))
    var empty: Set[Int] = Set[Int]()
    print(Bool(s), Bool(empty))
    a.clear()
    print(len(a))
    try:
        empty.remove(1)
    except:
        print("caught")
    try:
        var v = empty.pop()
        print(v)
    except e:
        print(e)
