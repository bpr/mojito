# The receiver and index of an in-place subscript are evaluated exactly once:
# the side-effecting index prints a single time.
@fieldwise_init
struct Counter(ImplicitlyCopyable, Movable):
    var value: Int

    def __iadd__(mut self, amount: Int):
        self.value += amount

@fieldwise_init
struct Row:
    var a: Counter

    def __getitem__(self, i: Int) -> Counter:
        return self.a

    def __setitem__(mut self, i: Int, v: Counter):
        self.a = v

def idx() -> Int:
    print("idx")
    return 0

def main():
    var r = Row(Counter(1))
    r[idx()] += 40
    print(r.a.value)
