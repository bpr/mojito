# A fieldwise-constructible struct value freezes at compile time: the
# factory call runs through VM-backed CTFE, field reads fold to constants,
# and the frozen instance materializes back as an ordinary construction.
from layout import IntTuple, Layout

comptime L = Layout.row_major(2, 3)
comptime T = IntTuple(4, 5)
comptime S0 = L.stride.d0
comptime R = L.shape.rank

def main():
    print(L)
    print(T)
    print(S0)
    print(R)
