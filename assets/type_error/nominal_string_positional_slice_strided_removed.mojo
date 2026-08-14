# expect: String positional slicing was removed
# The strided spelling is positional String slicing too: the whole surface
# was removed, not just the contiguous form. (StringLiteral slicing keeps
# the builtin literal behavior.)
def main():
    var s: String = "hello"
    print(s[::-1])
