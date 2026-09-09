# expect: escapes storage
# A nominal callable struct carrying a reference field is just a
# reference-bearing aggregate: storing one rooted at a method-local into
# `self` is the same store-outward escape, with no callable special-casing.
@fieldwise_init
struct Peek[origin: Origin[mut=True]](def() -> Int):
    var target: ref[origin] Int
    def __call__(self) -> Int:
        return self.target

@fieldwise_init
struct Holder[origin: Origin[mut=True]]:
    var slot: Peek[Self.origin]
    def swap(mut self):
        var local = 9
        ref alias = local
        self.slot = Peek(alias)

def main():
    var keep = 4
    ref a = keep
    var holder = Holder(Peek(a))
    holder.swap()
