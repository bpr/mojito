# A generic `def`'s abstract body may call a `def` whose `comptime if` keys on
# its own type parameter, inferred (`show(x)`) or explicit (`show[T](x)`): the
# abstract call checks against the callee's signature, and each instantiation
# of the caller reaches the callee's clone.

def show[T: Copyable](x: T):
    comptime if T == Int:
        print("int")
    else:
        print("other")

def pick[T: ImplicitlyCopyable & Writable & Deinitable](x: T) -> T:
    comptime if T == Int:
        print("pick int")
    return x

def unused[T: Copyable](x: T):
    show(x)
    show[T](x)

def forward[T: ImplicitlyCopyable & Writable & Deinitable](x: T):
    show(x)
    show[T](x)
    print(pick(x))

def outer[U: ImplicitlyCopyable & Writable & Deinitable](y: U):
    forward(y)

def relay[T: ImplicitlyCopyable & Writable & Deinitable](x: T) -> T:
    return pick[T](x)

def main():
    outer(3)
    outer(True)
    print(relay(4))
    print(relay(String("s")))
