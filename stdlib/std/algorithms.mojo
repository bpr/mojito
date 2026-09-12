# Small self-hosted generic helpers that deliberately exercise comptime facts.

from std.iterable import Iterable

trait StaticSized:
    comptime size: Int

def type_tag[T: AnyType]() -> Int:
    comptime if T == Int:
        return 1
    elif T == String:
        return 2
    else:
        return 0

def next_pow2(n: Int) -> Int:
    var x: Int = 1
    while x < n:
        x = x * 2
    return x

comptime DEFAULT_CAP = next_pow2(5)

def default_capacity() -> Int:
    return DEFAULT_CAP

def capacity_blocks[blocks: Int]() -> Int:
    return DEFAULT_CAP * blocks

def static_size[T: StaticSized]() -> Int:
    return T.size

def first_or[C: Iterable](items: C, default: C.Element) -> C.Element:
    for item in items:
        return item.copy()
    return default.copy()
