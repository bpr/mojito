# expect: out of range
# TypeList indexing is bounds-checked at compile time.
def main():
    comptime tl = TypeList.of[Int, Bool]()
    comptime third = tl[2]
    print(1)
