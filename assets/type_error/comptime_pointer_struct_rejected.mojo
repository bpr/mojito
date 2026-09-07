# expect: needs fieldwise construction
# A pointer-owning struct cannot freeze as a compile-time value: the VM
# constructs it, but the result has no fieldwise compile-time form.
comptime S = String("hello")

def main():
    print(S)
