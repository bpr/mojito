# A free `def` whose `comptime if` keys on a type parameter specializes per
# inferred call: `show(3)` selects the `Int` arm without spelling `show[Int]`.

def show[T: Copyable](x: T):
    comptime if T == Int:
        print("int")
    else:
        print("other")

def pick[T: ImplicitlyCopyable & Writable & Deinitable](x: T) -> T:
    comptime if T == Int:
        print("pick int")
    return x

def outer[T: ImplicitlyCopyable & Writable & Deinitable](x: T):
    comptime if T == Float64:
        print("outer float")
    show(x)
    print(pick(x))

def forward[T: ImplicitlyCopyable & Writable & Deinitable](x: T):
    show(x)

def count[T: Copyable](n: Int, x: T) -> Int:
    comptime if T == Int:
        if n == 0:
            return 0
        return 1 + count(n - 1, x)
    else:
        return -1

def main():
    show(3)
    show[Int](4)
    show(5)
    show(2.5)
    show(String("s"))
    outer[Int](7)
    outer[Float64](1.5)
    forward(True)
    forward(8)
    var y = pick(9)
    print(y)
    print(count(3, 1))
    print(count(3, 1.0))
