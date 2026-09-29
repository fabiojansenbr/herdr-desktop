// Files area (spec 070): the local files workspace, the read-only remote (SFTP) workspace, the
// review screen they share (header, tabs, side-by-side diff, terminal dock) and the words the
// files reducers put in a tab — what was saved, what changed on disk and which read a snapshot is.
//
// File names, paths and contents are the engine's, never translated. The remote hints explain a
// coded failure next to the engine's own message; they are the product's words, not the engine's.

import { area } from "../index.svelte";

export default area({
  en: {
    "files.explorer.title": "Files",
    "files.explorer.label": "File explorer",
    "files.explorer.refresh": "Refresh",
    "files.explorer.tree": "Project files",
    "files.explorer.noProject": "Choose a project to see its files.",
    "files.explorer.loading": "Loading files…",
    "files.explorer.emptyFolder": "Empty folder.",
    "files.explorer.emptyRoot":
      "No file in this root. The editor and the language modules load only when a text file is opened.",
    "files.explorer.loadMore": "Load more",
    "files.explorer.loadingDir": "Loading…",

    "files.workspace.label": "File editor",
    "files.workspace.editorNotLoaded": "The editor was not loaded for this file.",
    "files.workspace.loadingFile": "Loading file…",
    "files.workspace.save": "Save",
    "files.workspace.diffOriginal": "Diff vs. original",
    "files.workspace.external": "The file changed on disk since it was read.",
    "files.workspace.saving": "Saving…",
    "files.workspace.conflict": "File conflict",
    "files.workspace.reload": "Reload",
    "files.workspace.compare": "Compare",
    "files.workspace.saveCopy": "Save a copy",
    "files.workspace.recovery": "Copy kept at {path}",
    "files.workspace.closePrompt": "Close tab with unsaved changes",
    "files.workspace.unsaved": "This tab has unsaved changes.",
    "files.workspace.saveAndClose": "Save and close",
    "files.workspace.discard": "Discard",
    "files.workspace.cancel": "Cancel",
    "files.workspace.empty":
      "Open a file. No file open: the editor and the language modules load only when a file is opened.",

    "files.notice.saved": "Saved.",
    "files.notice.recoveryCopy": "Recovery copy saved at {path}.",
    "files.notice.reloaded": "Reloaded from disk.",
    "files.diff.baseDisk": "disk version {id}",
    "files.diff.baseOriginal": "base content {id}",
    "files.diff.current": "current buffer",

    "files.tabs.label": "Open files",
    "files.tabs.dirty": "unsaved",
    "files.tabs.close": "Close {name}",
    "files.tabs.openFile": "+ Open file",

    "files.review.title": "Review changes",
    "files.review.breadcrumb": "File path",
    "files.review.base": "Previous snapshot",
    "files.review.current": "Current version",
    "files.review.identical": "No differences",
    "files.review.dock": "Terminal of the active pane",
    "files.review.dockClose": "Close the terminal dock",
    "files.review.dockEmpty": "No confirmed pane on this host.",
    "files.review.noAgent": "no agent",
    "files.review.noProject": "no project",

    "files.remote.title": "Remote files",
    "files.remote.hosts": "SSH hosts",
    "files.remote.tree": "Remote project files",
    "files.remote.workspace": "Remote file",
    "files.remote.banner": "{host} · SSH · Read-only · Snapshot comparison",
    "files.remote.noHost": "Choose an SSH host to see the remote project's files.",
    "files.remote.hostOffline":
      "{host} is {phase}. Connect the host to list files; open tabs show the cache only.",
    "files.remote.noRoot": "No project root authorised for this host.",
    "files.remote.loading": "Loading remote files…",
    "files.remote.treeStale": "The connection changed: this tree is from the previous connection. Use Refresh.",
    "files.remote.loadingFile": "Reading remote file…",
    "files.remote.compare": "Compare with the previous read",
    "files.remote.staleOffline": "that is not active",
    "files.remote.stalePrevious": "previous",
    "files.remote.stale": "Outdated: content cached from {snapshot}, read on a connection {reason}; it is not the host's current state.",
    "files.remote.staleReload": "Reload to read it again.",
    "files.remote.empty":
      "Open a file. No remote file open: remote files open read-only; the editor loads only when a file is opened.",
    "files.remote.snapshot": "read {id} · generation {generation}",

    "files.badge.host": "Source SSH host",
    "files.badge.readOnly": "Read-only",
    "files.badge.stale": "Outdated",

    "files.hint.sftp_unavailable":
      "A working SSH does not guarantee SFTP: this host's terminals stay usable, but sshd offers no sftp subsystem. Ask for it to be enabled (Subsystem sftp) and refresh.",
    "files.hint.host_unavailable": "The host is disconnected; what you see is cache, not live state.",
    "files.hint.remote_name_unsupported":
      "This folder has a file name outside UTF-8, which this version does not list; other folders stay available.",
    "files.hint.cancelled":
      "Only this host's file channel was closed; the open content and the terminals were preserved.",
    "files.hint.ssh": "Resolve the SSH connection in a terminal; the desktop accepts no keys and asks for no password.",
  },
  pt: {
    "files.explorer.title": "Arquivos",
    "files.explorer.label": "Explorador de arquivos",
    "files.explorer.refresh": "Atualizar",
    "files.explorer.tree": "Arquivos do projeto",
    "files.explorer.noProject": "Escolha um projeto para ver os arquivos.",
    "files.explorer.loading": "Carregando arquivos…",
    "files.explorer.emptyFolder": "Pasta vazia.",
    "files.explorer.emptyRoot":
      "Nenhum arquivo nesta raiz. O editor e os módulos de linguagem só carregam quando um arquivo de texto é aberto.",
    "files.explorer.loadMore": "Carregar mais",
    "files.explorer.loadingDir": "Carregando…",

    "files.workspace.label": "Editor de arquivos",
    "files.workspace.editorNotLoaded": "O editor não foi carregado para este arquivo.",
    "files.workspace.loadingFile": "Carregando arquivo…",
    "files.workspace.save": "Salvar",
    "files.workspace.diffOriginal": "Diff vs. original",
    "files.workspace.external": "O arquivo mudou no disco desde a leitura.",
    "files.workspace.saving": "Salvando…",
    "files.workspace.conflict": "Conflito no arquivo",
    "files.workspace.reload": "Recarregar",
    "files.workspace.compare": "Comparar",
    "files.workspace.saveCopy": "Salvar cópia",
    "files.workspace.recovery": "Cópia preservada em {path}",
    "files.workspace.closePrompt": "Fechar aba com alterações não salvas",
    "files.workspace.unsaved": "Esta aba tem alterações não salvas.",
    "files.workspace.saveAndClose": "Salvar e fechar",
    "files.workspace.discard": "Descartar",
    "files.workspace.cancel": "Cancelar",
    "files.workspace.empty":
      "Abra um arquivo. Nenhum arquivo aberto: o editor e os módulos de linguagem carregam só ao abrir um arquivo.",

    "files.notice.saved": "Salvo.",
    "files.notice.recoveryCopy": "Cópia de recuperação salva em {path}.",
    "files.notice.reloaded": "Recarregado do disco.",
    "files.diff.baseDisk": "versão no disco {id}",
    "files.diff.baseOriginal": "conteúdo-base {id}",
    "files.diff.current": "buffer atual",

    "files.tabs.label": "Arquivos abertos",
    "files.tabs.dirty": "não salvo",
    "files.tabs.close": "Fechar {name}",
    "files.tabs.openFile": "+ Abrir arquivo",

    "files.review.title": "Revisar alterações",
    "files.review.breadcrumb": "Caminho do arquivo",
    "files.review.base": "Snapshot anterior",
    "files.review.current": "Versão atual",
    "files.review.identical": "Sem diferenças",
    "files.review.dock": "Terminal do pane ativo",
    "files.review.dockClose": "Fechar o dock do terminal",
    "files.review.dockEmpty": "Sem pane confirmado neste host.",
    "files.review.noAgent": "sem agente",
    "files.review.noProject": "sem projeto",

    "files.remote.title": "Arquivos remotos",
    "files.remote.hosts": "Hosts SSH",
    "files.remote.tree": "Arquivos do projeto remoto",
    "files.remote.workspace": "Arquivo remoto",
    "files.remote.banner": "{host} · SSH · Somente leitura · Comparação entre snapshots",
    "files.remote.noHost": "Escolha um host SSH para ver os arquivos do projeto remoto.",
    "files.remote.hostOffline":
      "{host} está {phase}. Conecte o host para listar arquivos; abas abertas mostram só o cache.",
    "files.remote.noRoot": "Nenhuma raiz de projeto autorizada para este host.",
    "files.remote.loading": "Carregando arquivos remotos…",
    "files.remote.treeStale": "A conexão mudou: esta árvore é da conexão anterior. Use Atualizar.",
    "files.remote.loadingFile": "Lendo arquivo remoto…",
    "files.remote.compare": "Comparar com leitura anterior",
    "files.remote.staleOffline": "que não está ativa",
    "files.remote.stalePrevious": "anterior",
    "files.remote.stale": "Desatualizado: conteúdo em cache da {snapshot}, lido numa conexão {reason}; não é o estado atual do host.",
    "files.remote.staleReload": "Recarregue para ler de novo.",
    "files.remote.empty":
      "Abra um arquivo. Nenhum arquivo remoto aberto: arquivos remotos abrem em modo somente leitura; o editor carrega só ao abrir um arquivo.",
    "files.remote.snapshot": "leitura {id} · geração {generation}",

    "files.badge.host": "Host SSH de origem",
    "files.badge.readOnly": "Somente leitura",
    "files.badge.stale": "Desatualizado",

    "files.hint.sftp_unavailable":
      "SSH funcionando não garante SFTP: os terminais deste host continuam utilizáveis, mas o sshd não oferece o subsistema sftp. Peça para habilitá-lo (Subsystem sftp) e atualize.",
    "files.hint.host_unavailable": "O host está desconectado; o que aparece é cache e não estado vivo.",
    "files.hint.remote_name_unsupported":
      "Esta pasta tem nome de arquivo fora de UTF-8, que esta versão não lista; outras pastas seguem disponíveis.",
    "files.hint.cancelled":
      "Só o canal de arquivos deste host foi encerrado; o conteúdo aberto e os terminais foram preservados.",
    "files.hint.ssh": "Resolva a conexão SSH em um terminal; o desktop não aceita chaves nem pede senha.",
  },
  es: {
    "files.explorer.title": "Archivos",
    "files.explorer.label": "Explorador de archivos",
    "files.explorer.refresh": "Actualizar",
    "files.explorer.tree": "Archivos del proyecto",
    "files.explorer.noProject": "Elige un proyecto para ver sus archivos.",
    "files.explorer.loading": "Cargando archivos…",
    "files.explorer.emptyFolder": "Carpeta vacía.",
    "files.explorer.emptyRoot":
      "Ningún archivo en esta raíz. El editor y los módulos de lenguaje se cargan solo al abrir un archivo de texto.",
    "files.explorer.loadMore": "Cargar más",
    "files.explorer.loadingDir": "Cargando…",

    "files.workspace.label": "Editor de archivos",
    "files.workspace.editorNotLoaded": "El editor no se cargó para este archivo.",
    "files.workspace.loadingFile": "Cargando archivo…",
    "files.workspace.save": "Guardar",
    "files.workspace.diffOriginal": "Diff vs. original",
    "files.workspace.external": "El archivo cambió en el disco desde la lectura.",
    "files.workspace.saving": "Guardando…",
    "files.workspace.conflict": "Conflicto en el archivo",
    "files.workspace.reload": "Recargar",
    "files.workspace.compare": "Comparar",
    "files.workspace.saveCopy": "Guardar copia",
    "files.workspace.recovery": "Copia conservada en {path}",
    "files.workspace.closePrompt": "Cerrar pestaña con cambios sin guardar",
    "files.workspace.unsaved": "Esta pestaña tiene cambios sin guardar.",
    "files.workspace.saveAndClose": "Guardar y cerrar",
    "files.workspace.discard": "Descartar",
    "files.workspace.cancel": "Cancelar",
    "files.workspace.empty":
      "Abre un archivo. Ningún archivo abierto: el editor y los módulos de lenguaje se cargan solo al abrir un archivo.",

    "files.notice.saved": "Guardado.",
    "files.notice.recoveryCopy": "Copia de recuperación guardada en {path}.",
    "files.notice.reloaded": "Recargado desde el disco.",
    "files.diff.baseDisk": "versión en disco {id}",
    "files.diff.baseOriginal": "contenido base {id}",
    "files.diff.current": "búfer actual",

    "files.tabs.label": "Archivos abiertos",
    "files.tabs.dirty": "sin guardar",
    "files.tabs.close": "Cerrar {name}",
    "files.tabs.openFile": "+ Abrir archivo",

    "files.review.title": "Revisar cambios",
    "files.review.breadcrumb": "Ruta del archivo",
    "files.review.base": "Instantánea anterior",
    "files.review.current": "Versión actual",
    "files.review.identical": "Sin diferencias",
    "files.review.dock": "Terminal del panel activo",
    "files.review.dockClose": "Cerrar el dock del terminal",
    "files.review.dockEmpty": "Ningún panel confirmado en este host.",
    "files.review.noAgent": "sin agente",
    "files.review.noProject": "sin proyecto",

    "files.remote.title": "Archivos remotos",
    "files.remote.hosts": "Hosts SSH",
    "files.remote.tree": "Archivos del proyecto remoto",
    "files.remote.workspace": "Archivo remoto",
    "files.remote.banner": "{host} · SSH · Solo lectura · Comparación entre instantáneas",
    "files.remote.noHost": "Elige un host SSH para ver los archivos del proyecto remoto.",
    "files.remote.hostOffline":
      "{host} está {phase}. Conecta el host para listar archivos; las pestañas abiertas muestran solo la caché.",
    "files.remote.noRoot": "Ninguna raíz de proyecto autorizada para este host.",
    "files.remote.loading": "Cargando archivos remotos…",
    "files.remote.treeStale": "La conexión cambió: este árbol es de la conexión anterior. Usa Actualizar.",
    "files.remote.loadingFile": "Leyendo archivo remoto…",
    "files.remote.compare": "Comparar con la lectura anterior",
    "files.remote.staleOffline": "que no está activa",
    "files.remote.stalePrevious": "anterior",
    "files.remote.stale": "Desactualizado: contenido en caché de {snapshot}, leído en una conexión {reason}; no es el estado actual del host.",
    "files.remote.staleReload": "Recarga para leerlo de nuevo.",
    "files.remote.empty":
      "Abre un archivo. Ningún archivo remoto abierto: los archivos remotos se abren en solo lectura; el editor se carga al abrir un archivo.",
    "files.remote.snapshot": "lectura {id} · generación {generation}",

    "files.badge.host": "Host SSH de origen",
    "files.badge.readOnly": "Solo lectura",
    "files.badge.stale": "Desactualizado",

    "files.hint.sftp_unavailable":
      "Un SSH que funciona no garantiza SFTP: los terminales de este host siguen utilizables, pero sshd no ofrece el subsistema sftp. Pide que se habilite (Subsystem sftp) y actualiza.",
    "files.hint.host_unavailable": "El host está desconectado; lo que ves es caché, no estado vivo.",
    "files.hint.remote_name_unsupported":
      "Esta carpeta tiene un nombre de archivo fuera de UTF-8, que esta versión no lista; las demás carpetas siguen disponibles.",
    "files.hint.cancelled":
      "Solo se cerró el canal de archivos de este host; el contenido abierto y los terminales se conservaron.",
    "files.hint.ssh": "Resuelve la conexión SSH en un terminal; el escritorio no acepta claves ni pide contraseña.",
  },
});
