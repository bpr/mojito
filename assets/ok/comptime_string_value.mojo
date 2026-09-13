# A compile-time `String` freezes its text; each runtime use rematerializes it.
comptime S = String("hello")

def main():
    print(S)
