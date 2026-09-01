# A conformer with the migrated-List receiver shape: a borrowed
# `__iter__(ref self)` overloaded with an owned `__iter__(var self)`, reached
# through generic trait dispatch. The abstract dispatch symbol pins one
# borrowed receiver spelling, and the overload makes the arity fallback
# ambiguous, so the VM must probe the sibling `ref self` spelling.

@fieldwise_init
struct StopIteration:
    pass

trait CountIterator:
    comptime Element: Movable

    def __next__(mut self) raises StopIteration -> Self.Element:
        ...

trait CountIterable:
    comptime Element: Copyable & Movable
    comptime Iter: CountIterator

    def __iter__(ref self) -> Self.Iter:
        ...

@fieldwise_init
struct CountIter(Copyable, CountIterator, Deinitable, Movable):
    comptime Element = Int

    var current: Int
    var stop: Int

    def __len__(self) -> Int:
        return self.stop - self.current

    def __next__(mut self) raises StopIteration -> Int:
        if self.current >= self.stop:
            raise StopIteration()
        var value = self.current
        self.current += 1
        return value

@fieldwise_init
struct Counter(Copyable, CountIterable, Deinitable, Movable):
    comptime Element = Int
    comptime Iter = CountIter

    var stop: Int

    def __iter__(ref self) -> CountIter:
        return CountIter(0, self.stop)

    def __iter__(var self) -> CountIter:
        return CountIter(0, self.stop)

def first_count[C: CountIterable](items: C, default: C.Element) -> C.Element:
    for item in items:
        return item.copy()
    return default.copy()

def main():
    print(first_count(Counter(3), -1))
    print(first_count(Counter(0), -1))
