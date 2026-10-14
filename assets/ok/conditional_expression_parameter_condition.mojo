# A conditional expression whose condition is a parameter value is
# `hlcf.comptime.if`: each instance keeps only the live arm, so the dead
# arm's recursive call is never instantiated.
comptime ZERO = 0


def is_zero(n: Int) -> Bool:
    return n == 0


def fact[n: Int]() -> Int:
    return 1 if n == ZERO else n * fact[n - 1]()


def count_down[n: Int]() -> Int:
    return n + count_down[n - 1]() if not (n == 0) else 0


def local_done[n: Int]() -> Int:
    comptime done = n <= 0
    return 1 if done else n * local_done[n - 1]()


def evaluated[n: Int]() -> Int:
    return 1 if comptime(is_zero(n)) else n * evaluated[n - 1]()


def flag_count[flag: Bool, n: Int]() -> Int:
    return 0 if not flag else 1 + flag_count[n > 1, n - 1]()


def copy_or_one[T: AnyType, n: Int]() -> Int:
    return 1 if conforms_to(T, Copyable) else n * copy_or_one[T, n - 1]()


def label[flag: Bool]() -> String:
    return String("on") if flag else String("off")


struct Counter[on: Bool, n: Int]:
    @staticmethod
    def total() -> Int:
        return 0 if not Self.on else Self.n + Counter[Self.n > 1, Self.n - 1].total()


def main():
    print(fact[5](), count_down[4](), local_done[4](), evaluated[4]())
    print(flag_count[True, 3](), copy_or_one[Int, 3]())
    print(label[True](), label[False](), Counter[True, 4].total())
    comptime for i in range(3):
        print("first" if i == 0 else "later")
