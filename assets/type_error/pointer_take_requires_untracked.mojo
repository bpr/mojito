# Taking the pointee deinitializes storage, which an origin-bearing pointer
# does not own — only an allocation-owning untracked pointer may.
# expect: mutable untracked origin
def main():
    var x = 1
    var p = Pointer(to=x)
    var v = p.unsafe_take_pointee()
