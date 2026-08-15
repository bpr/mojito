@fieldwise_init
struct Counter(ImplicitlyCopyable, Copyable, Movable):
    var value: Int

    def __iadd__(mut self, amount: Int):
        self.value += amount

@fieldwise_init
struct Row:
    var a: Counter
    var b: Counter

    def __getitem__(self, i: Int) -> Counter:
        return self.a if i == 0 else self.b

    def __setitem__(mut self, i: Int, v: Counter):
        if i == 0:
            self.a = v
        else:
            self.b = v

@fieldwise_init
struct Box:
    var item: Counter

    def __getitem__(mut self, index: Int) -> ref[origin_of(self.item)] Counter:
        return self.item

def main():
    var r = Row(Counter(1), Counter(2))
    r[0] += 40
    r[1] += 5
    print(r[0].value, r[1].value)
    var box = Box(Counter(10))
    box[0] += 3
    print(box.item.value)
