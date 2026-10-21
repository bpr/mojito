# expect: has no method '__getitem__'
# Each element is read through the value's `__getitem__`, which `S` lacks.
struct S(Sized, Copyable):
    def __init__(out self):
        pass
    def __len__(self) -> Int:
        return 2
def main() raises:
    var [a, b] = S()
    print(a, b)
