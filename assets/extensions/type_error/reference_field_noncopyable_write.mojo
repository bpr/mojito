# expect: cannot copy non-Copyable
# A write-through of a non-Copyable value through a `ref` field would need a
# copy of the referent; require an explicit transfer instead.
struct Sole(Movable):
    var n: Int
    def __init__(out self, n: Int):
        self.n = n

@fieldwise_init
struct RefCell[origin: Origin[mut=True]]:
    var value: ref[origin] Sole
    def put(mut self, var replacement: Sole):
        self.value = replacement

def main():
    var keep = Sole(1)
    ref whole = keep
    var cell = RefCell(whole)
    cell.put(Sole(9))
