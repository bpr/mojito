# A call through a `def(...)` bound is the bound's `__call__` requirement
# call: a struct runs its conformance's witness, a function value and a
# closure their own call.


@fieldwise_init
struct Counter(def(Int) -> Int, Copyable, Movable):
    var base: Int

    def __call__(self, x: Int) -> Int:
        return self.base + x


@fieldwise_init
struct Keeper(def(mut List[Int], var String) -> Int, Copyable, Movable):
    var base: Int

    def __call__(self, mut items: List[Int], var label: String) -> Int:
        items.append(self.base + label.byte_length())
        return len(items)


def apply[F: def(Int) -> Int](f: F, x: Int) -> Int:
    return f(x)


def apply_over[T: Copyable, F: def(T) -> T](f: F, x: T) -> T:
    return f(x)


def double(x: Int) -> Int:
    return x * 2


def push_one(mut items: List[Int], var label: String) -> Int:
    items.append(label.byte_length())
    return len(items)


def drive[
    F: def(mut List[Int], var String) -> Int
](f: F, mut items: List[Int]) -> Int:
    var name = String("abc")
    var n = f(items, name^)
    return n + f(items, String("hello"))


def risky(x: Int) raises -> Int:
    if x < 0:
        raise Error("negative")
    return x + 1


def try_it[F: def(Int) raises -> Int](f: F, x: Int) -> Int:
    try:
        return f(x)
    except e:
        print(e)
        return -1


def main():
    print(apply(Counter(10), 5))
    print(apply(double, 5))
    var k = 3

    def add_k(x: Int) {imm k} -> Int:
        return x + k

    print(apply(add_k, 5))
    print(apply_over(Counter(10), 6))
    print(apply_over(double, 6))
    print(apply_over(add_k, 6))
    var items = List[Int]()
    print(drive(Keeper(10), items))
    print(drive(push_one, items))
    print(len(items))
    print(try_it(risky, 1))
    print(try_it(risky, -1))
