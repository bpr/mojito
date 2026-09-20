# Compile-time machine `Int` addition wraps at 64 bits while exact literal
# arithmetic stays exact.
def main():
    comptime a = Int(9223372036854775807) + Int(1)
    comptime exact = (9223372036854775807 + 1) == 9223372036854775808
    print(a)
    print(exact)
