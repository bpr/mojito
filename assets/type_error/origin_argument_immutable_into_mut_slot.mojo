# expect: a mutable origin for parameter 'o' of 'TakeIter'
# A slot declared Origin[mut=True] rejects a provably immutable argument.
@fieldwise_init
struct TakeIter[o: Origin[mut=True]]:
    var src: ref[o] List[Int]

    def get(self) -> Int:
        return self.src[0]

struct Box:
    var items: List[Int]

    def __init__(out self):
        self.items = List[Int]()
        self.items.append(1)

    def snap(self) -> Int:
        ref source = self.items
        var t = TakeIter[origin_of(source)](source)
        return t.get()

def main():
    var b = Box()
    print(b.snap())
