# The reference-getter assignment fallback still requires a mutable receiver:
# a read-only parameter yields an immutable element reference.
# expect: must be mutable
def touch(a: Array[Int, 2]):
    a[0] = 1

def main():
    var a: Array[Int, 2] = [1, 2]
    touch(a)
