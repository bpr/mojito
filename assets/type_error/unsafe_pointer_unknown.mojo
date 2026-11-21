# The pin declares no `UnsafePointer`; a pointer is spelled `Pointer`.
# expect: unknown type 'UnsafePointer'
def f(p: UnsafePointer[Int, _]) -> Int:
    return p[]


def main():
    var x = 5
    print(f(Pointer(to=x)))
