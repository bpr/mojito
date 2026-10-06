# A `comptime` binding in the body of a template-served `comptime for` — a
# parameter expression over the loop variable, a literal, an alias of a type
# binder — is bound once with the variable symbolic and decided per
# iteration, in a generic `def` and in a method of a generic struct.
@fieldwise_init
struct Stepper[n: Int]:
    var base: Int

    def show(self):
        comptime for i in range(Self.n):
            comptime j = i * 2
            print(self.base + j)


def doubled[n: Int]():
    comptime for i in range(n):
        comptime j = i * 2
        print(j)
        comptime if j == 2:
            print("two")
        comptime k = 7
        print(k + i)


def aliased[T: AnyType, n: Int]():
    comptime for i in range(n):
        comptime U = T
        comptime j = i + 1
        print(j)


def main():
    doubled[3]()
    doubled[1]()
    aliased[Int, 2]()
    Stepper[2](10).show()
    Stepper[3](20).show()
