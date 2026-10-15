# expect: conflicting inline level from a previous '@no_inline' decorator
struct Counter:
    var count: Int

    def __init__(out self):
        self.count = 0

    @always_inline
    @no_inline
    def bump(mut self):
        self.count += 1

def main():
    var counter = Counter()
    counter.bump()
    print(counter.count)
