# A generic comptime alias may be defined by a compile-time value; each use
# evaluates the body with the application's arguments.
comptime Twice[n: Int] = 2 * n

def main():
    print(Twice[3])
    print(Twice[Twice[5]])
