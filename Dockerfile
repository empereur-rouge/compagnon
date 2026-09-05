# syntax=docker/dockerfile:1
#
# Image de livraison du service.
#
# Deux étages : un constructeur qui porte toute la chaîne de compilation Rust (~2 Gio), et une
# image d'exécution qui ne porte que le binaire. La surface exposée à Internet n'a alors ni
# compilateur, ni gestionnaire de paquets utilisable, ni interpréteur d'outillage.

ARG VERSION_RUST=1.91
ARG VERSION_DEBIAN=bookworm

# ---------------------------------------------------------------------------------------
# Construction
# ---------------------------------------------------------------------------------------
FROM rust:${VERSION_RUST}-${VERSION_DEBIAN} AS constructeur

# cmake et clang compilent `aws-lc-sys`, la partie C de la pile cryptographique que rustls
# utilise par défaut — donc la TLS de tous les appels sortants vers Telegram. Sans ces deux
# paquets, la construction échoue sur un « cmake introuvable » plusieurs minutes après son
# début. Pas de nettoyage des listes apt : cet étage est jeté entier.
RUN apt-get update \
 && apt-get install --yes --no-install-recommends cmake clang

WORKDIR /source
COPY Cargo.toml Cargo.lock ./
COPY src ./src

# `--locked` interdit toute résolution différente de celle qui a passé les tests : sur une
# machine de construction, un Cargo.lock mis à jour en silence est une livraison non testée.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/source/target,sharing=locked \
    cargo build --release --locked --bin compagnon \
 && cp target/release/compagnon /compagnon

# ---------------------------------------------------------------------------------------
# Exécution
# ---------------------------------------------------------------------------------------
FROM debian:${VERSION_DEBIAN}-slim AS execution

# ca-certificates : reqwest vérifie les certificats via le magasin de la plateforme
#   (rustls-platform-verifier), qui lit /etc/ssl/certs. Sans ce paquet, tout appel sortant
#   échoue — y compris le `getMe` du démarrage, ce qui a au moins le mérite d'être bruyant.
#
# Pas de tzdata : la phase 0 ne raisonne sur aucune heure locale. Le jour où l'état de
# relation dépendra de « il est deux heures du matin chez elle » (phase 2), il faudra
# l'ajouter — et ce sera un changement conscient, pas un héritage.
RUN apt-get update \
 && apt-get install --yes --no-install-recommends ca-certificates \
 && rm --recursive --force /var/lib/apt/lists/*

# Utilisateur sans privilèges, sans mot de passe, sans interpréteur de commandes.
RUN useradd --system --uid 10001 --user-group --home-dir /app --shell /usr/sbin/nologin compagnon

WORKDIR /app
COPY --from=constructeur /compagnon /usr/local/bin/compagnon
USER compagnon

# Pas de VOLUME : la phase 0 n'écrit rien sur disque. La file vit en mémoire et son contenu
# est vidé à l'extinction, pas persisté. La phase 1 introduira la base, et avec elle le
# volume — le déclarer d'avance donnerait un répertoire vide que personne ne sauvegarderait.

EXPOSE 8080

# La sonde est portée par le binaire (`compagnon sonde`) : l'image ne contient ni curl ni
# wget, et l'adresse interrogée est déduite d'ADRESSE_ECOUTE au lieu d'être recopiée ici, où
# elle divergerait en silence.
#
#   --start-interval=1s  Le service est prêt en ~0,2 s, le temps d'un aller-retour `getMe`.
#                        Sans ce réglage, Docker attend 5 s avant la première sonde, et le
#                        proxy — qui dépend de sa santé — retient d'autant la terminaison TLS
#                        à chaque déploiement.
#   --start-period=15s   Marge large pour un `getMe` sur un réseau lent, sans être une
#                        fenêtre d'aveuglement.
#   --interval=60s       Rythme de croisière. Plus court ne sert personne : rien ne réagit
#                        automatiquement à un conteneur déclaré malade.
HEALTHCHECK --interval=60s --timeout=10s --start-period=15s --start-interval=1s --retries=3 \
    CMD ["compagnon", "sonde"]

ENTRYPOINT ["compagnon"]
