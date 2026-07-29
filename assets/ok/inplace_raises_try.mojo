# A raising in-place dunder participates in ordinary `try` handling; on the
# non-raising path its `mut self` mutation is committed to the receiver.
@fieldwise_init
struct Counter(Copyable, Movable):
    var value: Int

    def __iadd__(mut self, amount: Int) raises:
        if amount < 0:
            raise Error("negative")
        self.value += amount

def main():
    var c = Counter(0)
    try:
        c += 5
        c += 7
    except e:
        print("caught")
    print(c.value)
