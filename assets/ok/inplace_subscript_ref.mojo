# A mutable-reference subscript getter applies the element's in-place dunder
# through the reference handle, with no `__setitem__`.
@fieldwise_init
struct Counter(Copyable, Movable):
    var value: Int

    def __iadd__(mut self, amount: Int):
        self.value += amount

@fieldwise_init
struct Box:
    var item: Counter

    def __getitem__(mut self, index: Int) -> ref[origin_of(self.item)] Counter:
        return self.item

def main():
    var box = Box(Counter(1))
    box[0] += 40
    print(box.item.value)
