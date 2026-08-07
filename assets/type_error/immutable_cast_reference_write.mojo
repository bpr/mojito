# expect: must be mutable
# `Origin[mut=False].cast_from[o]` pins a yielded reference read-only even
# over a mutable source, so a `for ref` write through the binding rejects.
@fieldwise_init
struct StopIteration:
    pass

@fieldwise_init
struct NumbersIter[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int
    def __next__(mut self) raises StopIteration -> ref[Origin[mut=False].cast_from[o]] Int:
        if self.index >= len(self.src):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[r]

struct Numbers:
    var items: List[Int]
    def __init__(out self):
        self.items = [4, 5, 6]
    def __iter__(ref self) -> NumbersIter:
        ref items = self.items
        return NumbersIter(items, 0)

def main():
    var nums = Numbers()
    for ref x in nums:
        x += 10
