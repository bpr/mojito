# P3: parametric-mut origin param on a struct; application without explicit Bool.
from std.iterable import StopIteration


@fieldwise_init
struct PIter[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int

    def __next__(mut self) raises StopIteration -> ref[o] Int:
        if self.index >= len(self.src):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[r]


struct Numbers:
    var items: List[Int]

    def __init__(out self):
        self.items = [4, 5, 6]

    def __iter__(ref self) -> PIter:
        ref items = self.items
        return PIter(items, 0)


def main():
    var nums = Numbers()
    var total = 0
    for x in nums:
        total += x
    print(total)
