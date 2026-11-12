# Recursion through a method runs at any depth, as through a free function:
# each method call is one more frame, a `mut self` receiver a handle into
# the caller's storage.


struct Walker:
    var step: Int

    def __init__(out self, step: Int):
        self.step = step

    def down(self, n: Int) -> Int:
        if n > 0:
            return self.down(n - 1) + self.step
        return 0


struct Counter:
    var hits: Int

    def __init__(out self):
        self.hits = 0

    def bump(mut self, n: Int):
        if n > 0:
            self.hits += 1
            self.bump(n - 1)


def main():
    print(Walker(1).down(20000))
    var c = Counter()
    c.bump(20000)
    print(c.hits)
