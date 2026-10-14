# expect: to runtime because it is not 'ImplicitlyCopyable'
# A value pack loop materializes each element through its iterator, which a
# pack of merely `Copyable` structs cannot do implicitly.
@fieldwise_init
struct Q(Copyable, Writable):
    var a: Int

    def write_to(self, mut w: Some[Writer]):
        w.write("Q(", self.a, ")")


def qs[*ps: Q]():
    comptime for p in ps:
        print(p)


def main():
    qs[Q(1), Q(2)]()
