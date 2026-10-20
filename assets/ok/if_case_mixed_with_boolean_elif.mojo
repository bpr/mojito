# A pattern branch between boolean branches: the right-hand side runs only
# when the branch is reached, and its binding is visible in its body alone.
def get() -> Tuple[Int, Int]:
    print("get")
    return (1, 2)


def main():
    var flag = False
    if flag:
        print("flag")
    elif case (1, var b) = get():
        print("b", b)
        b += 1
    elif case (2, _) = get():
        print("two")
    else:
        print("else")
