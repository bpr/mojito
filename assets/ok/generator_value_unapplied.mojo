# A parameterized `comptime` declaration read unapplied is a generator value
# (upstream's `#kgen.gen`), which a later application binds: a trait
# requirement through a bound, a struct's member, a module alias, and a
# struct's own member through `Self`, bound locally or at module scope.
trait HasGet:
    comptime get[idx: Int]: Int


struct A(HasGet):
    comptime get[idx: Int]: Int = idx * 2


struct Scaled[k: Int]:
    comptime times[x: Int]: Int = x * Self.k

    @staticmethod
    def show():
        comptime t = Self.times
        print(t[5])


comptime Twice[x: Int] = x * 2
comptime Add[a: Int, b: Int] = a + b
comptime twice_gen = Twice
comptime get_gen = A.get


def through_bound[T: HasGet]():
    comptime g = T.get
    print(g[4])


def main():
    through_bound[A]()
    comptime h = A.get
    print(h[5])
    comptime t = Twice
    print(t[4])
    comptime add = Add
    print(add[3, b=4])
    Scaled[3].show()
    comptime s = Scaled[10].times
    print(s[2])
    print(twice_gen[6], get_gen[7])
