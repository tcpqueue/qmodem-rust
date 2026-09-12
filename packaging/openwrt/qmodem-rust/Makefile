# Keep this package inside the repository, or pass QMODEM_SOURCE_DIR explicitly.
QMODEM_SOURCE_DIR ?= $(abspath $(dir $(lastword $(MAKEFILE_LIST)))/../../..)

include $(TOPDIR)/rules.mk

PKG_NAME:=qmodem-rust
PKG_VERSION:=0.1.0
PKG_RELEASE:=4
PKG_LICENSE:=GPL-3.0-only
ifeq ($(strip $(QMODEM_PREBUILT_BIN)),)
PKG_BUILD_DEPENDS:=rust/host
endif
PKG_BUILD_PARALLEL:=1
PKG_BUILD_DIR:=$(BUILD_DIR)/$(PKG_NAME)-$(PKG_VERSION)

include $(INCLUDE_DIR)/package.mk
ifeq ($(strip $(QMODEM_PREBUILT_BIN)),)
include $(TOPDIR)/feeds/packages/lang/rust/rust-package.mk
endif

RUST_PKG_LOCKED:=1
MAKE_PATH:=crates/qmodemd

define Package/qmodem-rust
  SECTION:=net
  CATEGORY:=Network
  TITLE:=QModem Rust modem service
  DEPENDS:=+ubus +netifd +firewall4
endef

define Package/qmodem-rust/description
 Native Rust modem service with TOML settings and SQLite storage.
 Development version; Quectel and TD Tech MT5700 compatibility is in progress.
endef

define Package/qmodem-rust/conffiles
/etc/qmodem-rust.toml
endef

define Build/Prepare
	$(call Build/Prepare/Default)
	$(CP) $(QMODEM_SOURCE_DIR)/Cargo.toml $(QMODEM_SOURCE_DIR)/Cargo.lock $(QMODEM_SOURCE_DIR)/NOTICE.md $(PKG_BUILD_DIR)/
	$(CP) $(QMODEM_SOURCE_DIR)/crates $(QMODEM_SOURCE_DIR)/config $(QMODEM_SOURCE_DIR)/data $(QMODEM_SOURCE_DIR)/licenses $(PKG_BUILD_DIR)/
	$(INSTALL_DIR) $(PKG_BUILD_DIR)/frontend
	$(CP) $(QMODEM_SOURCE_DIR)/frontend/dist $(PKG_BUILD_DIR)/frontend/
endef

ifneq ($(strip $(QMODEM_PREBUILT_BIN)),)
define Build/Configure
endef

define Build/Compile
	test -f "$(QMODEM_PREBUILT_BIN)"
	$(QMODEM_SOURCE_DIR)/scripts/check-openwrt-binary.sh "$(QMODEM_PREBUILT_BIN)" "$(ARCH)"
	$(INSTALL_DIR) $(PKG_INSTALL_DIR)/bin
	$(INSTALL_BIN) "$(QMODEM_PREBUILT_BIN)" $(PKG_INSTALL_DIR)/bin/qmodemd
endef

define Build/Install
endef
endif

define Package/qmodem-rust/install
	$(INSTALL_DIR) $(1)/usr/sbin $(1)/etc/init.d
	$(INSTALL_BIN) $(PKG_INSTALL_DIR)/bin/qmodemd $(1)/usr/sbin/qmodemd
	$(INSTALL_BIN) ./files/qmodem-rust.init $(1)/etc/init.d/qmodem-rust
	$(INSTALL_CONF) $(QMODEM_SOURCE_DIR)/config/qmodem.example.toml $(1)/etc/qmodem-rust.toml
	$(INSTALL_DIR) $(1)/usr/share/licenses/qmodem-rust
	$(CP) $(QMODEM_SOURCE_DIR)/NOTICE.md $(QMODEM_SOURCE_DIR)/licenses/* $(1)/usr/share/licenses/qmodem-rust/
endef

define Package/qmodem-rust-qmi
  SECTION:=net
  CATEGORY:=Network
  TITLE:=QModem USB QMI protocol dependencies
  DEPENDS:=+qmodem-rust +uqmi +kmod-usb-net-qmi-wwan
endef

define Package/qmodem-rust-mbim
  SECTION:=net
  CATEGORY:=Network
  TITLE:=QModem USB MBIM protocol dependencies
  DEPENDS:=+qmodem-rust +umbim +kmod-usb-net-cdc-mbim
endef

define Package/qmodem-rust-qmi/install
	$(INSTALL_DIR) $(1)/usr/share/qmodem-rust
endef
define Package/qmodem-rust-mbim/install
	$(INSTALL_DIR) $(1)/usr/share/qmodem-rust
endef

$(eval $(call BuildPackage,qmodem-rust))
$(eval $(call BuildPackage,qmodem-rust-qmi))
$(eval $(call BuildPackage,qmodem-rust-mbim))
