struct Counter(Copyable, Movable):
    var count: Int

    def __init__(out self, start: Int):
        self.count = start

    def bump(mut self, by: Int):
        self.count = self.count + by

    def value(self) -> Int:
        return self.count


def main():
    var c = Counter(5)
    c.bump(2)
    c.bump(3)
    print(c.value(), c.count)
    var d = c
    d.bump(90)
    print(c.count, d.count)
