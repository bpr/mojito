# expect: cannot use a dynamic value in operator argument
def main():
    var x = True
    comptime i = x and True
    print(i)
