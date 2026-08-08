# A literal string argument prefers an exact overload, else converts to
# the nominal String overload through the @implicit literal constructor.
def pick(x: Int) -> Int:
    return x

def pick(x: String) -> Int:
    return len(x)

def main():
    print(pick(7), pick("four"))
