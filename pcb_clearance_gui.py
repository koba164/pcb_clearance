"""インストール不要 exe と Python GUI の共通起動点。"""
import multiprocessing

if __name__ == "__main__":
    # 子プロセスを GUI 初期化より前に分岐する。
    multiprocessing.freeze_support()
    from pcb_clearance.gui import main
    main()
