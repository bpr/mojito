# A capturing closure's body is a method of its storage struct, whose
# parameters are the enclosing binders the closure names: in its body
# (`n`, a type binder `T`, a `comptime for` index) as in its fields, while
# its own (`m`) stay the method's. A call by name calls the method on the
# storage, a closure value is a closure over the method, and a body owning
# a moved capture writes the storage it is called on, in place.


def seven() -> Int:
    return 7


def generic_frame[n: Int](v: Int) -> Int:
    var x = v

    def body() {var x} -> Int:
        return x + n

    def sig[m: Int](y: Int) {var x} -> Int:
        return x + m + y

    return body() + sig[7](1)


def callable_frame[f: def() thin -> Int](v: Int) -> Int:
    var x = v

    def body() {var x} -> Int:
        return x + f()

    return body()


def type_frame[T: ImplicitlyCopyable & Writable & Deinitable](w: Int):
    var x = w

    def show() {var x}:
        var local: List[T] = []
        print(len(local), x)

    show()


def loop_frame() -> Int:
    var total = 0
    var x = 1
    comptime for i in range(3):

        def step() {var x} -> Int:
            return x + i

        total += step()
    return total


def twice[F: def() -> Int](f: F) -> Int:
    return f() * 10 + f()


def def_value[n: Int](v: Int) -> Int:
    var x = v

    def body() {var x} -> Int:
        return x + n

    return twice(body)


def lambda_value[n: Int](v: Int) -> Int:
    var x = v
    return twice(lambda {var x} -> Int: x + n)


def string_state() -> String:
    var s = String("a")

    def grow() {var s^} -> String:
        s += "b"
        return s

    def both() {grow} -> String:
        return grow() + grow()

    return both() + grow()


def main():
    print(generic_frame[3](10))
    print(callable_frame[seven](1))
    type_frame[String](2)
    print(loop_frame())
    print(def_value[3](1))
    print(lambda_value[3](1))
    print(string_state())
