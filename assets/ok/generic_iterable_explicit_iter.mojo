# An explicit `__iter__()` on a generic iterable is typed
# `C.IteratorType[origin_of(items)]`, whose `Iterator` bound answers
# `__next__` and `Element` (upstream's own `iter()` signature).
def first[
    C: Iterable
](ref items: C) raises StopIteration -> C.IteratorType[origin_of(items)].Element:
    var it = items.__iter__()
    return it.__next__()


def it_of[C: Iterable](ref items: C) -> C.IteratorType[origin_of(items)]:
    return items.__iter__()


def main():
    var xs: List[Int] = [4, 5, 6]
    var s: List[String] = ["a", "b"]
    try:
        print(first(xs))
        print(first(s))
        var it = it_of(xs)
        print(it.__next__())
        print(it.__next__())
    except:
        print("empty")
