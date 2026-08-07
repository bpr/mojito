# expect: access to 'other' conflicts with live reference 'holder'
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Holder:
    var slot: RefBox
    def rebind_to(mut self, mut source: List[Int]):
        ref alias = source
        self.slot = RefBox(alias)

def main():
    var keep = [1]
    ref whole = keep
    var holder = Holder(RefBox(whole))
    var other = [5]
    holder.rebind_to(other)
    other.append(6)
    print(holder.slot.value[0])
