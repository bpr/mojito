# A store to the owner while an origin-bearing pointer to it is live runs,
# as at the pin: the pointer names the owner's storage, which the store
# writes in place, so it reads 5 (R1160).
def main():
    var x = 1
    var p = Pointer(to=x)
    x = 5
    print(p[])
