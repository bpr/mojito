# An explicit `^` transfer through a `ref` field moves the value into the
# referent without a copy, and reads that project past the ref field route
# through the handle-chasing reference walk.
struct SoleList(Movable):
    var items: List[Int]
    def __init__(out self, n: Int):
        self.items = [n]

@fieldwise_init
struct RefCell[origin: Origin[mut=True]]:
    var value: ref[origin] SoleList
    def put(mut self, var replacement: SoleList):
        self.value = replacement^
    def first(self) -> Int:
        return self.value.items[0]

def main():
    var keep = SoleList(1)
    ref whole = keep
    var cell = RefCell(whole)
    cell.put(SoleList(9))
    print(cell.first())
