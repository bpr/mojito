# expect: incompatible types across alternatives
# A name bound by every `|` alternative has one type.
def main():
    match (1, "s"):
        case (var n, "t") | (1, var n):
            print("x")
