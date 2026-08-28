@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Holder[origin: Origin[mut=True]]:
    var slot: RefBox[Self.origin]
    def rebind_to(mut self, mut source: List[Int]):
        ref alias = source
        self.slot = RefBox(alias)

def main():
    var keep: List[Int] = [1]
    ref whole = keep
    var holder = Holder(RefBox(whole))
    var other: List[Int] = [5]
    holder.rebind_to(other)
    print(holder.slot.value[0])
    other.append(6)
    print(other[1])
