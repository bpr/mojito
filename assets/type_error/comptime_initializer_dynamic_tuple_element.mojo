# expect: cannot use a dynamic value in 'ref' argument
def main():
    var x = 3
    comptime i = (x, 1)
    print(i[0])
