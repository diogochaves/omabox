// omabox-qtext: what a Qt Quick program in a box draws as text, for `omabox tree --layer` and `look
// plugin` (#467, NOTES finding 727). Preloaded into the box's shell only (share/shell.sh and the
// systemd-cat stand-in: LD_PRELOAD on Quickshell's exec), never on the host: it does nothing outside a
// box (omabox_inside_box).
//
// Qt Quick publishes an item over AT-SPI only when its QML gives it an Accessible role or name (Qt 6.12's
// QQuickText has no accessibility of its own), and Omarchy's bar and most plugins give none, so `tree`
// found nothing in the bar. This library reads the items themselves instead, in the program: once
// QGuiApplication starts (Q_COREAPP_STARTUP_FUNCTION) it listens on
// $XDG_RUNTIME_DIR/omabox-qtext/PID, and each connection gets, read on the GUI thread, every window
//
//   window<TAB>W<TAB>H<TAB>ACTIVE<TAB>NAMESPACE
//   text<TAB>X<TAB>Y<TAB>W<TAB>H<TAB>CUT<TAB>TEXT
//
// NAMESPACE is a Quickshell layer's (its proxy's `namespace` property; empty for anything else); a text
// line is a visible Text, TextInput (its displayText: a password stays masked) or TextEdit under the
// window before it, with any opacity left, not clipped away, in the window's coordinates, its tabs and
// line breaks as spaces; CUT is 1 when part of it is outside the window or a clipping ancestor (a
// label wider than a vertical bar shows only its middle) or Qt elided it (`truncated`), else 0.
// Nothing is written into the program; a request is not read (any connection is one).
#include <QCoreApplication>
#include <QGuiApplication>
#include <QQuickItem>
#include <QQuickWindow>
#include <QSocketNotifier>
#include <QTextDocumentFragment>
#include <QVariant>
#include <QWindow>

#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>

extern "C" {
#include "../common/box.h"
}

static std::string sock_path;
static int listen_fd = -1;

// Our own entry out of LD_PRELOAD, the rest kept (a box with --polkit preloads its session shim): what
// the shell starts (apps from its menus, helpers) must not load Qt Quick for this. Done once the
// QGuiApplication is there, not as the library loads: Quickshell executes itself again as it starts,
// and that second image is the one to read.
static void unpreload() {
    const char *v = getenv("LD_PRELOAD");
    if (!v) return;
    std::string kept, all(v);
    size_t at = 0;
    while (at <= all.size()) {
        size_t end = all.find_first_of(": ", at);
        if (end == std::string::npos) end = all.size();
        std::string e = all.substr(at, end - at);
        if (!e.empty() && e.find("omabox-qtext") == std::string::npos) kept += (kept.empty() ? "" : ":") + e;
        at = end + 1;
    }
    if (kept.empty()) unsetenv("LD_PRELOAD");
    else setenv("LD_PRELOAD", kept.c_str(), 1);
}

static void flat(QString &s) {
    for (QChar &c : s)
        if (c == u'\t' || c == u'\n' || c == u'\r' || c == QChar::LineSeparator || c == QChar::ParagraphSeparator) c = u' ';
}

static bool inherits_any(const QObject *o, std::initializer_list<const char *> names) {
    for (const char *n : names)
        if (o->inherits(n)) return true;
    return false;
}

// What ITEM draws as text, or a null string when it draws none.
static QString text_of(QQuickItem *item) {
    QString s;
    if (inherits_any(item, {"QQuickTextInput"})) s = item->property("displayText").toString();
    else if (inherits_any(item, {"QQuickText", "QQuickTextEdit"})) {
        s = item->property("text").toString();
        // (Text.StyledText, RichText or AutoText that looks rich: the words, not the markup)
        int fmt = item->property("textFormat").toInt();
        if (fmt == Qt::RichText || ((fmt == Qt::AutoText || fmt == 2 /* StyledText */) && Qt::mightBeRichText(s)))
            s = QTextDocumentFragment::fromHtml(s).toPlainText();
    } else return QString();
    flat(s);
    return s.trimmed();
}

static void walk(QQuickItem *item, qreal opacity, const QRectF &clip, std::string &out, int depth) {
    if (!item || !item->isVisible() || depth > 200) return;
    opacity *= item->opacity();
    if (opacity < 0.01) return;
    QRectF r = item->mapRectToScene(QRectF(0, 0, item->width(), item->height()));
    QRectF in = clip;
    if (item->clip()) in = clip.intersected(r);
    QString s = text_of(item);
    if (!s.isEmpty() && r.width() > 0 && r.height() > 0 && clip.intersects(r)) {
        QByteArray t = s.toUtf8();
        char head[96];
        // (a pixel either way: rounding; or a Text elided or cut to its lines: `truncated`)
        int cut = !clip.adjusted(-1, -1, 1, 1).contains(r) || item->property("truncated").toBool();
        snprintf(head, sizeof head, "text\t%d\t%d\t%d\t%d\t%d\t", qRound(r.x()), qRound(r.y()), qRound(r.width()),
                 qRound(r.height()), cut);
        out += head;
        out.append(t.constData(), t.size());
        out += '\n';
    }
    if (in.isEmpty() && item->clip()) return;
    for (QQuickItem *c : item->childItems()) walk(c, opacity, in, out, depth + 1);
}

// A Quickshell layer's namespace: its window's content item is parented to the PanelWindow (the
// proxy), whose `namespace` property is the layer's.
static QString namespace_of(QQuickWindow *w) {
    for (QQuickItem *c : w->contentItem()->childItems()) {
        QObject *proxy = c->parent();
        if (!proxy) continue;
        QVariant ns = proxy->property("namespace");
        if (ns.isValid()) return ns.toString();
    }
    return QString();
}

static std::string dump() {
    std::string out;
    for (QWindow *w : QGuiApplication::allWindows()) {
        auto *q = qobject_cast<QQuickWindow *>(w);
        if (!q || !q->isVisible()) continue;
        QString ns = namespace_of(q);
        flat(ns);
        char head[64];
        snprintf(head, sizeof head, "window\t%d\t%d\t%d\t", q->width(), q->height(), q->isActive() ? 1 : 0);
        out += head;
        out += ns.toUtf8().constData();
        out += '\n';
        walk(q->contentItem(), 1.0, QRectF(0, 0, q->width(), q->height()), out, 0);
    }
    return out;
}

static void answer(int lfd) {
    int c = accept4(lfd, nullptr, nullptr, SOCK_CLOEXEC);
    if (c < 0) return;
    struct timeval tv = {2, 0};
    setsockopt(c, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof tv);
    std::string s = dump();
    for (size_t off = 0; off < s.size();) {
        ssize_t n = send(c, s.data() + off, s.size() - off, MSG_NOSIGNAL);
        if (n <= 0 && errno != EINTR) break;
        if (n > 0) off += n;
    }
    close(c);
}

static void start() {
    if (!qobject_cast<QGuiApplication *>(QCoreApplication::instance())) return;
    unpreload();
    if (!omabox_inside_box()) return;
    const char *rt = getenv("XDG_RUNTIME_DIR");
    if (!rt || !*rt || listen_fd >= 0) return;
    std::string dir = std::string(rt) + "/omabox-qtext";
    mkdir(dir.c_str(), 0700);
    sock_path = dir + "/" + std::to_string(getpid());
    struct sockaddr_un a = {};
    a.sun_family = AF_UNIX;
    if (sock_path.size() >= sizeof a.sun_path) { sock_path.clear(); return; }
    memcpy(a.sun_path, sock_path.c_str(), sock_path.size() + 1);
    int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
    if (fd < 0) { sock_path.clear(); return; }
    unlink(sock_path.c_str());
    if (bind(fd, (struct sockaddr *)&a, sizeof a) < 0 || listen(fd, 4) < 0) {
        close(fd);
        sock_path.clear();
        return;
    }
    listen_fd = fd;
    auto *n = new QSocketNotifier(fd, QSocketNotifier::Read, QCoreApplication::instance());
    QObject::connect(n, &QSocketNotifier::activated, [fd] { answer(fd); });
    // (Called again for each QCoreApplication a program makes: the socket goes with the one it was
    // made for, and the next one makes its own.)
    qAddPostRoutine([] {
        if (listen_fd >= 0) close(listen_fd), unlink(sock_path.c_str());
        listen_fd = -1;
    });
}
Q_COREAPP_STARTUP_FUNCTION(start)
