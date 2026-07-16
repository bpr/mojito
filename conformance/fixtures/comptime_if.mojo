def main():
    comptime width: Int = 8
    comptime if width > 4:
        print("wide")
    else:
        print("narrow")
