@fieldwise_init
struct ValueBox(Copyable, Movable):
    var value: Int

    def __getitem__(self, index: Int) -> Int:
        return self.value + index

    def __setitem__(mut self, index: Int, value: Int):
        self.value = value + index


@fieldwise_init
struct Outer(Copyable, Movable):
    var box: ValueBox

    def __getitem__(ref self, index: Int) -> ref[origin_of(self.box)] ValueBox:
        return self.box


def rhs() -> Int:
    print("rhs")
    return 40


def receiver_index() -> Int:
    print("receiver")
    return 0


def index() -> Int:
    print("index")
    return 2


@fieldwise_init
struct RefBox:
    var value: Int

    def __getitem__(ref self, index: Int) -> ref[origin_of(self.value)] Int:
        return self.value


def next_index() -> Int:
    print("next")
    return 0


def bump(mut value: Int):
    value += 2


trait IntIndexer:
    def __getitem__(self, index: Int) -> Int: ...


@fieldwise_init
struct Pair(IntIndexer):
    var first: Int
    var second: Int

    def __getitem__(self, index: Int) -> Int:
        if index == 0:
            return self.first
        return self.second


def second[T: IntIndexer](value: T) -> Int:
    return value[1]


def main():
    var outer = Outer(ValueBox(0))
    outer[receiver_index()][index()] = rhs()
    print(outer.box.value)

    var box = RefBox(40)
    bump(box[next_index()])
    print(box.value)

    print(second(Pair(3, 7)))
