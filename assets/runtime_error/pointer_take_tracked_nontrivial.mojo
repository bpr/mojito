# expect: use after Pointer deallocation
# Taking a pointee that owns storage through an origin-bearing Pointer moves
# the value out, as at the pin, but leaves `s` to be destroyed again by its
# checked owner after the pointer's last use: the VM traps the read of `v`'s
# freed storage, which the pin leaves undefined (it runs to exit 0). The
# String is built by `+`, so it owns a heap buffer (a literal-built one is
# static and frees nothing).
def main():
    var s = String("own") + "ed"
    var p = Pointer(to=s)
    var v = p.unsafe_take_pointee()
    print(v)
