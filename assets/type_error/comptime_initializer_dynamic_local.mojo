# expect: cannot use a dynamic value in comptime initializer
def main():
    var x = 3
    comptime i = x + 1
    print(i)
