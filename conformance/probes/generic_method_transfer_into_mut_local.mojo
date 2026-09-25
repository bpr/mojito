struct Bag[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def fill(self, mut target: List[Self.T], var value: Self.T):
        target.append(value^)

    def push_filled(mut self, var value: Self.T):
        var extra = List[Self.T]()
        self.fill(extra, value^)
        self.items = extra^


def main():
    var a = Bag[Int]()
    a.push_filled(3)
    print(len(a.items))
