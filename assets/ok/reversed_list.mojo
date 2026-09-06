# `reversed(list)` is upstream's free-function overload over a `List[T]` (a
# borrowed back-to-front iterator) beside the range overloads. A user overload
# set whose generic member spells a parameterized annotation (`List[T]`)
# mangles that member as its call sites do, so the erased body is reached.

def pick[T: Copyable](ref xs: List[T]) -> T:
    return xs[0].copy()


def pick[T: Copyable](ref xs: List[T], n: Int) -> T:
    return xs[n].copy()


def main():
    var xs: List[Int] = [1, 2, 3]
    for x in reversed(xs):
        print(x)
    var words: List[String] = ["a", "b"]
    for w in reversed(words):
        print(w)
    for r in reversed(range(2)):
        print(r)
    print(pick(xs), pick(xs, 2))
