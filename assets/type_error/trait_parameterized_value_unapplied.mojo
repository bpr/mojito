# A parameterized `comptime` value requirement read through a bound
# unapplied is a generator, which is not `Writable`.
# expect: with type '__generator_type[idx: Int] Int' does not conform to trait 'Writable'
trait HasGet:
    comptime get[idx: Int]: Int


def use[T: HasGet]():
    print(T.get)


def main():
    pass
