# A store to the owner while a `Pointer` to it is live runs on both: the
# pointer names the owner's origin and borrows nothing exclusively, so it
# reads the stored value, 5.
def main():
    var x = 1
    var p = Pointer(to=x)
    x = 5
    print(p[])
