# expect: must bind the same names
# Every `|` alternative must bind the same names.
def main():
    match (1, 2):
        case (var a, 2) | (1, var b):
            print(a)
