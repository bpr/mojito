# A parameterized `comptime` value requirement read through a bound is
# applied, never read whole.
# expect: expects 1 type argument(s), got 0
trait HasGet:
    comptime get[idx: Int]: Int


def use[T: HasGet]():
    print(T.get)


def main():
    pass
