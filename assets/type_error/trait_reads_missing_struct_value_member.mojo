# expect: 'P[Int, Bool]' value has no attribute 'm'
# A trait's signature reading a value member a parameterized struct does not
# declare is rejected once the module's structs have registered.


struct P[A: AnyType, B: AnyType]:
    comptime n = 3


struct Q[k: Int]:
    def __init__(out self):
        pass


trait Tr:
    def get(self, x: Q[P[Int, Bool].m]) -> Int:
        ...


def main():
    print(1)
