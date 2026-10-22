# A loop left by two `break`s, each storing a different value read after it:
# the loop's result enters the block after it from both edges, as an
# argument of that block once `LowerControlFlow` lowers `hlcf.loop`.
#
# Expected output:
#   4
#   100
#   -1


def first_over(limit: Int, n: Int) -> Int:
    var found = -1
    for i in range(n):
        if i * i > limit:
            found = i
            break
        if i == 7:
            found = 100
            break
    return found


def main():
    print(first_over(10, 20))
    print(first_over(1000, 20))
    print(first_over(1000, 3))
