# A conformer cannot inherit a default whose compile-time parameter shares a
# name with one of the struct's own parameters.
# expect: name conflict between parameter 'x' in the default trait method and a parameter in the struct
trait Foo:
    def foo[x: Int](self) -> Int:
        return x

struct Bar[x: Int](Foo):
    def __init__(out self):
        pass

def main():
    print(Bar[1]().foo[2]())
