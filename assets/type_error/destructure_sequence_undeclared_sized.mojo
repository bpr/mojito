# expect: value of type 'S' does not conform to 'Sized'
# The declared `Sized` conformance decides, not the presence of `__len__`.
struct S(Copyable):
    def __init__(out self):
        pass
    def __len__(self) -> Int:
        return 2
    def __getitem__(self, i: Int) -> Int:
        return i
def main() raises:
    var [a, b] = S()
    print(a, b)
