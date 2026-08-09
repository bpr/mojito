# expect: not safe for VM-backed compile-time execution
# A pointer-owning struct cannot freeze as a compile-time value; its
# constructors fail the CTFE purity walk.
comptime S = String("hello")

def main():
    print(S)
