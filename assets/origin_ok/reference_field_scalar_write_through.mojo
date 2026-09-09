# Assigning through a `Pointer`-typed field inside a `mut self` method writes
# through the stored handle into the caller-owned referent — the store twin
# of LoadPlace's second dereference — and the loan on the source stays live
# while the carrier does.
@fieldwise_init
struct RefCell[origin: Origin[mut=True]]:
    var value: Pointer[Int, Self.origin]
    def put(mut self, n: Int):
        self.value[] = n

def main():
    var slot = 1
    ref whole = slot
    var cell = RefCell(Pointer(to=whole))
    cell.put(42)
    print(cell.value[])
