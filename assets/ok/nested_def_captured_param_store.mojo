@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Holder:
    var slot: RefBox

    def stash_param(mut self, mut source: List[Int]):
        def install() {mut self, ref source}:
            ref alias = source
            self.slot = RefBox(alias)
        install()

def main():
    var keep = [1]
    ref whole = keep
    var holder = Holder(RefBox(whole))
    var other = [5]
    holder.stash_param(other)
    print("stored")
