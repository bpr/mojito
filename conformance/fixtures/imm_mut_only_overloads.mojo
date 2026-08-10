# Current Mojo treats parameter access conventions as call behavior, not
# overload identity: declarations differing only by `imm` versus `mut` reject.
def inspect(imm value: Int) -> Int:
    return value

def inspect(mut value: Int) -> Int:
    return value

def main():
    pass
