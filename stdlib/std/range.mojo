# Nominal Int range used by the compiler prelude.  Scalar/dtype-generic range
# overloads remain part of the later SIMD/scalar library slice.

from std.iterable import Iterable, Iterator, StopIteration

def _range_length(start: Int, stop: Int, step: Int) -> Int:
    if step == 0:
        return 0
    if step > 0:
        if start >= stop:
            return 0
        return (stop - start + step - 1) // step
    if start <= stop:
        return 0
    var stride = -step
    return (start - stop + stride - 1) // stride

@fieldwise_init
struct _RangeIter(Iterator):
    comptime Element = Int
    var current: Int
    var stop: Int
    var step: Int

    def __len__(self) -> Int:
        return _range_length(self.current, self.stop, self.step)

    def __next__(mut self) raises StopIteration -> Int:
        if self.step == 0:
            raise StopIteration()
        if self.step > 0 and self.current >= self.stop:
            raise StopIteration()
        if self.step < 0 and self.current <= self.stop:
            raise StopIteration()
        var result = self.current
        self.current += self.step
        return result

@fieldwise_init
struct Range(Copyable, ImplicitlyDeletable, Iterable, Movable, Writable):
    comptime Element = Int
    comptime Iter = _RangeIter
    var start: Int
    var stop: Int
    var step: Int

    def __len__(self) -> Int:
        return _range_length(self.start, self.stop, self.step)

    def __getitem__(self, index: Int) -> Int:
        return self.start + index * self.step

    def __contains__(self, value: Int) -> Bool:
        if self.step == 0:
            return False
        if self.step > 0:
            if value < self.start or value >= self.stop:
                return False
        else:
            if value > self.start or value <= self.stop:
                return False
        return (value - self.start) % self.step == 0

    def __iter__(self) -> _RangeIter:
        return _RangeIter(self.start, self.stop, self.step)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("range(", self.start, ", ", self.stop, ", ", self.step, ")")

def range(stop: Int) -> Range:
    return Range(0, stop, 1)

def range(start: Int, stop: Int) -> Range:
    return Range(start, stop, 1)

def range(start: Int, stop: Int, step: Int) -> Range:
    return Range(start, stop, step)
