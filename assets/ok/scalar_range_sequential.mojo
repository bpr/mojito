# The two-argument scalar range never counts down: `range(7, 3)` is empty,
# not descending (upstream's sequential-range rule). Elements are the
# argument dtype's scalars.
def main():
    for x in range(Int16(3), Int16(6)):
        print(x)
    print(len(range(Int32(7), Int32(3))))
    # A literal bound adopts the scalar argument's dtype.
    for y in range(Int64(8), 10):
        print(y)
