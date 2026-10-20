# expect: Undefined variable 'a'
# A case binding shadows an outer name inside its body only, and a tuple
# element binding is not visible after the match.
def main():
    var x = 1
    match 2:
        case x:
            print("inner", x)
    print("outer", x)
    match (3, 4):
        case (a, b):
            print(a, b)
    print(a)
