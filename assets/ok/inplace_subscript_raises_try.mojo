# A raising element in-place dunder participates in ordinary `try` handling; on
# the non-raising path the subscript element mutation is committed.
@fieldwise_init
struct Counter(ImplicitlyCopyable, Movable):
    var value: Int

    def __iadd__(mut self, amount: Int) raises:
        if amount < 0:
            raise Error("negative")
        self.value += amount

@fieldwise_init
struct Row:
    var a: Counter

    def __getitem__(self, i: Int) -> Counter:
        return self.a

    def __setitem__(mut self, i: Int, v: Counter):
        self.a = v

def main():
    var r = Row(Counter(1))
    try:
        r[0] += 40
    except e:
        print("caught")
    print(r.a.value)
