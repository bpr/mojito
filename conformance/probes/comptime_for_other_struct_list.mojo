# Pin gap probe (Mojo 1.2.0.dev2026092105): a `comptime for` over another
# struct's compile-time list member (`K.L`), in a plain function and in a
# generic struct's method. The pin prints `6` and `7 7`; Mojito reports
# "Undefined variable 'K'". Roadmap R513.
struct K:
    comptime L = [1, 2, 3]


struct S[T: AnyType]:
    var x: Int

    def __init__(out self):
        self.x = 1

    def get(self) -> Int:
        var t = self.x
        comptime for v in K.L:
            t += v
        return t


def main():
    var t = 0
    comptime for v in K.L:
        t += v
    print(t)
    print(S[Int]().get(), S[String]().get())
