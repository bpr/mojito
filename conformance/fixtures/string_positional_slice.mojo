# Positional String slicing was removed: the unit is spelled through the
# keyword slices, so `s[1:4]` rejects.
def main():
    var s = String("hello")
    print(s[1:4])
