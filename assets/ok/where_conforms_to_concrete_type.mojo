# A `where` clause's `conforms_to` over a concrete type is a fact the
# declaration decides.
def f[T: AnyType]() where conforms_to(Int, Copyable):
    print("ok")


def main():
    f[Int]()
