# A nested `def` or lambda declared in a `comptime for` body names the
# loop's index, in its body and its signature, without capturing it: each
# unrolled iteration declares its own, specialized to that iteration's
# index. So does one declared in an arm of a `comptime if`.


def apply[func: def() capturing -> Int]() -> Int:
    return func()


def in_generic[n: Int]():
    comptime for i in range(n):

        def inner():
            print("generic", i)

        inner()


def lambda_in_generic[n: Int]():
    comptime for i in range(n):
        var l = lambda -> Int: i + 7
        print(l())


def branch_in_body[n: Int]():
    comptime for i in range(n):

        def inner():
            comptime if i == 1:
                print("one")
            else:
                print("not one", i)

        inner()


def signature_over_index[n: Int]():
    comptime for i in range(n):

        def inner() -> SIMD[DType.int32, i + 1]:
            return SIMD[DType.int32, i + 1](Int32(i))

        print(inner())


def capture_beside_index[n: Int]():
    comptime for i in range(n):
        var base = 10

        def inner() {var base} -> Int:
            return base + i

        print(inner())


def passed_as_parameter[n: Int]():
    comptime for i in range(n):

        @parameter
        def inner() -> Int:
            return i + 100

        print(apply[inner]())


def generic_nested[n: Int]():
    comptime for i in range(n):

        def inner[k: Int]() -> Int:
            return i + k

        print(inner[5]())


def in_kept_arm[n: Int]():
    comptime if n == 2:

        def inner():
            print("two", n)

        inner()


@fieldwise_init
struct S(ImplicitlyCopyable):
    var v: Int

    def m(self):
        comptime for i in range(2):

            def inner():
                print("method", i)

            inner()


def main():
    in_generic[2]()
    lambda_in_generic[2]()
    branch_in_body[2]()
    signature_over_index[2]()
    capture_beside_index[2]()
    passed_as_parameter[2]()
    generic_nested[2]()
    in_kept_arm[2]()
    S(3).m()

    comptime for i in range(2):

        def inner():
            print("plain", i)

        inner()

    comptime for i in range(2):
        var l = lambda -> Int: i * 10
        print(l())

    comptime for i in range(2):

        def outer():
            comptime for j in range(2):

                def deeper():
                    print("nested", i, j)

                deeper()

        outer()

    comptime for i in range(3):
        comptime if i == 1:

            def only():
                print("only", i)

            only()
