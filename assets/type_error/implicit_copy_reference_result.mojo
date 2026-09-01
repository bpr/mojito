# expect: cannot be implicitly copied
# Binding a reference result as an owned value copies the referent; a
# Copyable-only referent needs `.copy()`.
@fieldwise_init
struct Holder:
    var items: List[Int]

    def get(ref self) -> ref[origin_of(self.items)] List[Int]:
        return self.items

def main():
    var holder = Holder(List[Int]())
    var duplicate = holder.get()
    print(len(duplicate))
