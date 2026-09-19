# A call result typed by a type parameter whose bounds do not prove
# `Deinitable` is an owned temporary nothing can implicitly destroy, so
# handing it to `print` — which only borrows it — abandons it. The body is
# keyed on its own parameter, so only source validation ever sees it with `T`
# symbolic: elaboration replaces the template with a trapping stub.
# expect: '(expression temporary)' abandoned without being explicitly destroyed
def pick[T: ImplicitlyCopyable & Writable](x: T) -> T:
    comptime if T == Int:
        pass
    return x

def outer[T: ImplicitlyCopyable & Writable](x: T):
    comptime if T == Float64:
        print("float")
    print(pick(x))

def main():
    outer[Int](7)
