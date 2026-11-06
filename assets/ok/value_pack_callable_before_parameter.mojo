# A callable value pack followed by another parameter: the pack declares
# its list and visible binders at its own source position, so `n` keeps
# its keyword and both read back in the body.


def hi():
    print("hi")


def bye():
    print("bye")


def run_all[*fs: def() thin -> None, n: Int]():
    fs[0]()
    fs[1]()
    print(n, len(fs))


def main():
    run_all[hi, bye, n=3]()
