# `StridedSlice` has no public bounds: upstream's one field is the private
# `_inner`, and the kind exposes only `indices()`. Both compilers reject
# the read; the pin says "'StridedSlice' value has no attribute 'start'".
from std.builtin.builtin_slice import StridedSlice

struct G:
    def __init__(out self):
        pass

    def __getitem__(self, s: StridedSlice) -> Int:
        return s.start.or_else(0)

def main():
    print(G()[1:5:2])
