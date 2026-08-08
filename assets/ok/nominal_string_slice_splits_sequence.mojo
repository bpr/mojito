# Byte-wise slicing may cut inside a multibyte UTF-8 sequence: the result
# keeps the raw bytes (len counts them) and prints lossily, matching the
# builtin literal slice.
def main():
    var s = String("héllo")
    var cut = s[0:2]
    print(len(cut))
    print(cut)
